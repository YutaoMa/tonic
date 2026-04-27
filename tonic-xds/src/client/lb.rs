use crate::client::cluster::ClusterClientRegistry;
use crate::client::endpoint::EndpointAddress;
use crate::client::route::RouteDecision;
use crate::common::async_util::BoxFuture;
use http::Request;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{BoxError, Service, ServiceExt, discover::Change};

/// A pinned, boxed stream of endpoint changes for Tower's `Discover`-based
/// load balancers. The factory layer (cluster registry) produces this from the
/// per-cluster connector and the address-only [`EndpointStream`].
pub(crate) type BoxDiscover<Endpoint, S> =
    Pin<Box<dyn futures_core::Stream<Item = Result<Change<Endpoint, S>, BoxError>> + Send>>;

/// Address-only EDS stream produced by [`ClusterDiscovery`] implementations.
/// The factory layer maps each [`EndpointDelta`] into a `Change<EndpointAddress, S>`
/// by applying the cluster's connector at Insert time.
pub(crate) type EndpointStream =
    Pin<Box<dyn futures_core::Stream<Item = Result<EndpointDelta, BoxError>> + Send>>;

/// An add/remove signal for a single endpoint address. Delivered by
/// [`ClusterDiscovery::discover_cluster`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EndpointDelta {
    Insert(EndpointAddress),
    Remove(EndpointAddress),
}

/// Trait for discovering cluster endpoints by name.
///
/// Implementations resolve a cluster name into a stream of [`EndpointDelta`]s
/// — pure EDS, no connection establishment. The factory layer (the cluster
/// client registry) is responsible for combining this stream with the cluster's
/// connector to produce the `Change<EndpointAddress, Service>` stream consumed
/// by the load balancer.
pub(crate) trait ClusterDiscovery: Send + Sync + 'static {
    fn discover_cluster(&self, cluster_name: &str) -> EndpointStream;
}

/// Errors that can occur during load balancing.
#[derive(Debug, Clone, thiserror::Error)]
pub(crate) enum LoadBalancingError {
    #[error("No routing decision extension from the routing layer available")]
    NoRoutingDecision,
}

/// A Tower Service that performs xDS-driven load balancing based on routing
/// decisions. Holds the [`ClusterClientRegistry`], which absorbs CDS lookup,
/// connector building, and EDS subscription per cluster.
pub(crate) struct XdsLbService<Req, Resp>
where
    Req: Send + 'static,
    Resp: 'static,
{
    cluster_registry: Arc<ClusterClientRegistry<Req, Resp>>,
}

impl<Req, Resp> XdsLbService<Req, Resp>
where
    Req: Send + 'static,
    Resp: 'static,
{
    pub(crate) fn new(cluster_registry: Arc<ClusterClientRegistry<Req, Resp>>) -> Self {
        Self { cluster_registry }
    }
}

impl<Req, Resp> Clone for XdsLbService<Req, Resp>
where
    Req: Send + 'static,
    Resp: 'static,
{
    fn clone(&self) -> Self {
        Self {
            cluster_registry: self.cluster_registry.clone(),
        }
    }
}

impl<B, Resp> Service<Request<B>> for XdsLbService<Request<B>, Resp>
where
    B: 'static,
    Request<B>: Send + 'static,
    Resp: Send + 'static,
    crate::client::endpoint::EndpointChannel<tonic::transport::Channel>:
        Service<Request<B>, Response = Resp> + Send + 'static,
    <crate::client::endpoint::EndpointChannel<tonic::transport::Channel> as Service<
        Request<B>,
    >>::Error: Into<BoxError>,
    <crate::client::endpoint::EndpointChannel<tonic::transport::Channel> as Service<
        Request<B>,
    >>::Future: Send + 'static,
{
    type Response = Resp;
    type Error = BoxError;
    type Future = BoxFuture<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Under xDS, the destination cluster is decided by the routing layer, which takes
        // the request as an input. Therefore, we cannot determine readiness without
        // knowing the target cluster, which is tied to the request.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let Some(routing_decision) = request.extensions().get::<RouteDecision>().cloned() else {
            return Box::pin(async move { Err(LoadBalancingError::NoRoutingDecision.into()) });
        };

        let cluster_client = self.cluster_registry.get_cluster(&routing_decision.cluster);
        let mut channel = cluster_client.channel();

        Box::pin(async move {
            // Blocks until the first endpoint is available for the cluster.
            channel.ready().await?;
            channel.call(request).await
        })
    }
}
