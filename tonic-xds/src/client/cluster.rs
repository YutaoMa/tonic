use crate::client::endpoint::{Connector, EndpointAddress, EndpointChannel};
use crate::client::lb::{BoxDiscover, ClusterDiscovery, EndpointDelta};
use crate::common::async_util::BoxFuture;
use crate::xds::cache::XdsCache;
use crate::xds::cert_provider::CertProviderRegistry;
use crate::xds::cluster_discovery::build_connector;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use futures_util::StreamExt as _;
use http::{Request, Response};
use std::fmt::Debug;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tonic::body::Body as TonicBody;
use tonic::transport::Channel;
use tower::discover::Change;
use tower::{
    BoxError, Service, balance::p2c::Balance, buffer::Buffer, discover::Discover, load::Load,
};

type RespFut<Resp> = BoxFuture<Result<Resp, BoxError>>;

const DEFAULT_BUFFER_CAPACITY: usize = 1024;

/// `ClusterBalancer` is responsible for managing load balancing requests across multiple channels.
/// Currently, `ClusterBalancer` leverges `tower::balance::p2c` for doing P2C load balancing. In the future, we will
/// support more load balancing strategies as needed.
pub(crate) struct ClusterBalancer<D, Req>
where
    D: Discover,
    D::Key: Hash,
{
    balancer: Balance<D, Req>,
}

impl<D, Req> ClusterBalancer<D, Req>
where
    D: Discover,
    D::Key: Hash,
    D::Service: Service<Req>,
    <D::Service as Service<Req>>::Error: Into<BoxError>,
{
    /// Creates a new `ClusterBalancer` with provided service discovery.
    pub(crate) fn new(discover: D) -> Self {
        Self {
            balancer: Balance::new(discover),
        }
    }

    /// Returns the number of endpoints currently tracked by the balancer.
    /// This can be useful for monitoring and debugging purposes.
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.balancer.len()
    }
}

impl<D, Req> Service<Req> for ClusterBalancer<D, Req>
where
    D: Discover + Unpin,
    D::Key: Hash + Clone,
    D::Error: Into<BoxError>,
    D::Service: Service<Req> + Load,
    <D::Service as Load>::Metric: std::fmt::Debug,
    <D::Service as Service<Req>>::Error: Into<BoxError> + 'static,
    <D::Service as Service<Req>>::Future: Send + 'static,
{
    type Response = <Balance<D, Req> as Service<Req>>::Response;
    type Error = <Balance<D, Req> as Service<Req>>::Error;
    type Future = RespFut<Self::Response>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.balancer.poll_ready(cx)
    }

    fn call(&mut self, req: Req) -> Self::Future {
        Box::pin(self.balancer.call(req))
    }
}

/// `ClusterChannel` is similar to `tonic::transport::Channel`, but is for load-balancing across all
/// the channels for a xDS Cluster.
/// `ClusterChannel` should be cloned to be used in multi-threaded environment. It leverages a `tower::Buffer` to
/// queue requests from multiple callers and behind the queue, it load-balances the requests across all
/// available channels by leveraging the inner `ClusterBalancer` object.
pub(crate) struct ClusterChannel<Req, Resp>
where
    Req: Send + 'static,
    Resp: 'static,
{
    // The mpsc channel between callers and the actual pool of channels.
    svc: Buffer<Req, BoxFuture<Result<Resp, BoxError>>>,
}

impl<Req, Resp> Clone for ClusterChannel<Req, Resp>
where
    Req: Send + 'static,
    Resp: 'static,
{
    fn clone(&self) -> Self {
        Self {
            svc: self.svc.clone(),
        }
    }
}

impl<Req, Resp> ClusterChannel<Req, Resp>
where
    Req: Send + 'static,
    Resp: 'static,
{
    /// Creates a new `ClusterChannel` with the given service and picker.
    pub(crate) fn from_balancer<B>(balancer: B, buffer_cap: usize) -> Self
    where
        B: Service<Req, Error = BoxError, Future = RespFut<Resp>> + Send + 'static,
    {
        let svc = Buffer::new(balancer, buffer_cap);
        Self { svc }
    }
}

impl<Req, Resp> Service<Req> for ClusterChannel<Req, Resp>
where
    Req: Send + 'static,
    Resp: 'static,
{
    type Response = Resp;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Service::poll_ready(&mut self.svc, cx).map_err(BoxError::from)
    }

    fn call(&mut self, request: Req) -> Self::Future {
        Box::pin(self.svc.call(request))
    }
}

/// `ClusterClient` manages channels that load-balance for a xDS cluster.
pub(crate) struct ClusterClient<Req, Resp>
where
    Req: Send + 'static,
    Resp: 'static,
{
    name: String,
    channel: ClusterChannel<Req, Resp>,
}

impl Debug for ClusterClient<(), ()> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterClient")
            .field("name", &self.name)
            .finish()
    }
}

impl<Req, Resp> ClusterClient<Req, Resp>
where
    Req: Send + 'static,
    Resp: 'static,
{
    /// Creates a new `ClusterClient` with the given cluster name and service discovery implementation.
    /// Currently, `tower::discover::Discover` is used for service discovery.
    pub(crate) fn new<D>(name: String, discover: D) -> Self
    where
        D: Discover + Unpin + Send + 'static,
        D::Key: std::hash::Hash + Clone + Send,
        D::Error: Into<BoxError>,
        D::Service: Service<Req, Response = Resp> + Load + Send + 'static,
        <D::Service as Load>::Metric: std::fmt::Debug,
        <D::Service as Service<Req>>::Error: Into<BoxError>,
        <D::Service as Service<Req>>::Future: Send + 'static,
    {
        let balancer = ClusterBalancer::new(discover);
        let channel = ClusterChannel::from_balancer(balancer, DEFAULT_BUFFER_CAPACITY);
        Self { name, channel }
    }

    /// Returns a channel that can be used to send RPCs to the cluster.
    pub(crate) fn channel(&self) -> ClusterChannel<Req, Resp> {
        self.channel.clone()
    }

    /// Returns the name of the cluster.
    #[allow(dead_code)]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

/// `ClusterClientRegistry` is the per-cluster cache + factory for xDS clusters.
///
/// On first lookup of a cluster name, the registry:
/// 1. Reads the current `ClusterResource` synchronously from [`XdsCache`]
/// 2. Builds a per-cluster [`Connector`] from the cluster's security config
/// 3. Spawns a CDS watcher that hot-swaps the connector on subsequent updates
/// 4. Subscribes to EDS via [`ClusterDiscovery`] and maps each endpoint
///    Insert/Remove delta to a Tower `Change<EndpointAddress, Service>` event,
///    applying the current connector at Insert time
///
/// Subsequent lookups return the cached `ClusterClient`.
pub(crate) struct ClusterClientRegistry<Req, Resp>
where
    Req: Send + 'static,
    Resp: 'static,
{
    registry: DashMap<String, Arc<ClusterClient<Req, Resp>>>,
    cache: Arc<XdsCache>,
    discovery: Arc<dyn ClusterDiscovery>,
    cert_provider_registry: Arc<CertProviderRegistry>,
}

impl<Req, Resp> ClusterClientRegistry<Req, Resp>
where
    Req: Send + 'static,
    Resp: 'static,
    EndpointChannel<Channel>: Service<Req, Response = Resp> + Send + 'static,
    <EndpointChannel<Channel> as Service<Req>>::Error: Into<BoxError>,
    <EndpointChannel<Channel> as Service<Req>>::Future: Send + 'static,
{
    pub(crate) fn new(
        cache: Arc<XdsCache>,
        discovery: Arc<dyn ClusterDiscovery>,
        cert_provider_registry: Arc<CertProviderRegistry>,
    ) -> Self {
        Self {
            registry: DashMap::new(),
            cache,
            discovery,
            cert_provider_registry,
        }
    }

    /// Returns the [`ClusterClient`] for `cluster_name`, building it on first
    /// lookup via [`build_cluster_stream`](Self::build_cluster_stream).
    pub(crate) fn get_cluster(&self, cluster_name: &str) -> Arc<ClusterClient<Req, Resp>> {
        self.registry
            .entry(cluster_name.to_string())
            .or_insert_with(|| {
                let stream = self.build_cluster_stream(cluster_name);
                Arc::new(ClusterClient::new(cluster_name.to_string(), stream))
            })
            .clone()
    }

    /// Build the per-cluster `Discover` stream: CDS-driven connector + EDS-driven
    /// addresses, mapped to `Change<EndpointAddress, EndpointChannel<Channel>>`.
    ///
    /// Spawns a background CDS watcher that hot-swaps the connector via
    /// [`ArcSwap`] when the `ClusterResource` changes. New endpoint Inserts
    /// pick up the new connector; in-flight handshakes complete with whatever
    /// connector they started with (eventual consistency on connection rotation).
    fn build_cluster_stream(
        &self,
        cluster_name: &str,
    ) -> BoxDiscover<EndpointAddress, EndpointChannel<Channel>> {
        // [1] Initial CDS read — fail fast if cluster is not in the cache.
        let Some(initial_cluster) = self.cache.get_cluster(cluster_name) else {
            return error_stream(format!("cluster '{cluster_name}' not in cache"));
        };

        // [2] Build the initial connector from CDS.
        let initial_connector =
            match build_connector(&initial_cluster, &self.cert_provider_registry) {
                Ok(c) => c,
                Err(e) => return error_stream(e.to_string()),
            };

        // [3] Hot-swappable shared reference. CDS watcher writes; address
        // mapping reads via load() at each Insert. The double Arc is needed
        // because ArcSwap's RefCnt impl requires the inner type to be Sized,
        // and `dyn Connector` isn't — we store `Arc<Arc<dyn Connector>>`.
        let connector_ref: Arc<ConnectorSlot> = Arc::new(ArcSwap::new(Arc::new(initial_connector)));

        // [4] CDS watcher — keeps connector_ref up-to-date over the cluster's
        // lifetime. Exits when the watch closes (cluster removed from cache).
        let mut cluster_watch = self.cache.watch_cluster(cluster_name);
        let cert_registry = self.cert_provider_registry.clone();
        let connector_for_watcher = connector_ref.clone();
        let cluster_name_owned = cluster_name.to_string();
        tokio::spawn(async move {
            // Skip the first value (already consumed via get_cluster above);
            // subsequent next() calls deliver actual updates.
            let _ = cluster_watch.next().await;
            while let Some(updated) = cluster_watch.next().await {
                match build_connector(&updated, &cert_registry) {
                    Ok(new) => connector_for_watcher.store(Arc::new(new)),
                    Err(e) => eprintln!(
                        "[tonic-xds] CDS update for cluster '{cluster_name_owned}' failed to \
                         build connector: {e}; keeping previous"
                    ),
                }
            }
        });

        // [5] Map address-only stream → Change<addr, service> via current connector.
        let addresses = self.discovery.discover_cluster(cluster_name);
        Box::pin(addresses.then(move |delta| {
            let connector_ref = connector_ref.clone();
            async move {
                match delta? {
                    EndpointDelta::Insert(addr) => {
                        // load_full returns Arc<Arc<dyn Connector>>; deref once.
                        let outer = connector_ref.load_full();
                        let connector = (*outer).clone();
                        let svc = connector.connect(&addr).await;
                        Ok(Change::Insert(addr, svc))
                    }
                    EndpointDelta::Remove(addr) => Ok(Change::Remove(addr)),
                }
            }
        }))
    }
}

/// Hot-swappable per-cluster connector. CDS updates store; address mapping loads.
/// The inner `Arc<dyn Connector>` isn't `Sized`, so `ArcSwap` requires it to be
/// wrapped in another `Arc` (which is sized).
type ConnectorSlot = ArcSwap<Arc<dyn Connector<Service = EndpointChannel<Channel>> + Send + Sync>>;

/// Single-element error stream — used when CDS lookup or connector build fails.
fn error_stream<S: Send + 'static>(msg: String) -> BoxDiscover<EndpointAddress, S> {
    Box::pin(futures_util::stream::once(async move {
        Err(BoxError::from(msg))
    }))
}

/// Type alias for the gRPC-flavored registry.
pub(crate) type ClusterClientRegistryGrpc =
    ClusterClientRegistry<Request<TonicBody>, Response<TonicBody>>;
