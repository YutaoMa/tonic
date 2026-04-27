//! xDS-backed [`ClusterDiscovery`] implementation.
//!
//! Builds a per-cluster [`Connector`] based on the cluster's
//! [`ClusterSecurityConfig`] (parsed from `Cluster.transport_socket`) and a
//! shared [`CertProviderRegistry`] (built from the bootstrap
//! `certificate_providers` map).

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{Channel, Endpoint};
use tower::BoxError;

use crate::client::endpoint::{Connector, EndpointAddress, EndpointChannel};
use crate::client::lb::{BoxDiscover, ClusterDiscovery};
use crate::common::async_util::BoxFuture;
use crate::xds::cache::XdsCache;
use crate::xds::cert_provider::CertProviderRegistry;
use crate::xds::endpoint_manager::EndpointManager;
use crate::xds::resource::ClusterResource;

const DISCOVER_CHANNEL_CAPACITY: usize = 64;

/// xDS-backed cluster discovery that resolves cluster names into endpoint
/// change streams by watching the [`XdsCache`].
pub(crate) struct XdsClusterDiscovery {
    cache: Arc<XdsCache>,
    cert_provider_registry: Arc<CertProviderRegistry>,
}

impl XdsClusterDiscovery {
    /// Creates a new `XdsClusterDiscovery`.
    pub(crate) fn new(
        cache: Arc<XdsCache>,
        cert_provider_registry: Arc<CertProviderRegistry>,
    ) -> Self {
        Self {
            cache,
            cert_provider_registry,
        }
    }
}

impl ClusterDiscovery<EndpointAddress, EndpointChannel<Channel>> for XdsClusterDiscovery {
    fn discover_cluster(
        &self,
        cluster_name: &str,
    ) -> BoxDiscover<EndpointAddress, EndpointChannel<Channel>> {
        let cache = self.cache.clone();
        let registry = self.cert_provider_registry.clone();
        let cluster_name = cluster_name.to_string();

        let (tx, rx) = mpsc::channel(DISCOVER_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            // Wait for the first ClusterResource to arrive so we can build a
            // connector matched to its security config. The cluster watch
            // closes if the cluster is removed from the cache, in which case
            // we exit silently — the receiver will see an empty stream.
            let mut cluster_watch = cache.watch_cluster(&cluster_name);
            let Some(cluster) = cluster_watch.next().await else {
                return;
            };

            let connector = match build_connector(&cluster, &registry) {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Err(Box::new(e) as BoxError)).await;
                    return;
                }
            };

            let manager = EndpointManager::new(connector);
            let mut stream = manager.discover_endpoints(cache.watch_endpoints(&cluster_name));
            while let Some(item) = stream.next().await {
                if tx.send(item).await.is_err() {
                    return;
                }
            }
        });

        Box::pin(ReceiverStream::new(rx))
    }
}

/// Build a [`Connector`] for a given cluster based on its security config.
///
/// Returns a [`PlaintextConnector`] when the cluster has no `transport_socket`.
/// When TLS is required, this currently returns
/// [`ConnectorBuildError::TlsNotWired`] — the rustls-side parsing/validation
/// (see [`crate::xds::cert_provider::verifier`]) is in place, but actually
/// plumbing a custom [`rustls::client::danger::ServerCertVerifier`] into a
/// `tonic::transport::Channel` requires `ClientTlsConfig::with_server_cert_verifier`,
/// which does not yet exist upstream.
fn build_connector(
    cluster: &ClusterResource,
    _registry: &Arc<CertProviderRegistry>,
) -> Result<
    Arc<dyn Connector<Service = EndpointChannel<Channel>> + Send + Sync>,
    ConnectorBuildError,
> {
    match &cluster.security {
        None => Ok(Arc::new(PlaintextConnector)),
        Some(_) => Err(ConnectorBuildError::TlsNotWired),
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConnectorBuildError {
    #[error(
        "data-plane TLS is not yet wired: tonic's ClientTlsConfig has no hook for \
         injecting a custom rustls::ServerCertVerifier (required for gRFC A29 SAN \
         matching). Tracking upstream"
    )]
    TlsNotWired,
}

/// Plaintext [`Connector`] producing a lazily-connected `tonic::Channel`.
pub(crate) struct PlaintextConnector;

impl Connector for PlaintextConnector {
    type Service = EndpointChannel<Channel>;

    fn connect(&self, addr: &EndpointAddress) -> BoxFuture<Self::Service> {
        let uri = format!("http://{addr}");
        // EndpointAddress only holds validated Ipv4/Ipv6/Hostname + u16
        // port, and its Display impl produces "ip:port" or "hostname:port".
        // Prefixing with "http://" always yields a valid URI.
        let channel = Endpoint::from_shared(uri)
            .expect("EndpointAddress Display guarantees valid URI")
            .connect_lazy();
        let result = EndpointChannel::new(channel);
        Box::pin(async move { result })
    }
}
