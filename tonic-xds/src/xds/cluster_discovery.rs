//! xDS-backed [`ClusterDiscovery`] implementation.
//!
//! Pure EDS subscription: turns a cluster name into a stream of
//! [`EndpointDelta`]s. CDS handling, connector building, and
//! address-to-service mapping live in the cluster registry's factory layer
//! (see [`ClusterClientRegistry`]).
//!
//! Also defines the per-cluster connector types — [`PlaintextConnector`] and
//! the `build_connector` helper used by the registry.
//!
//! [`ClusterDiscovery`]: crate::client::lb::ClusterDiscovery
//! [`EndpointDelta`]: crate::client::lb::EndpointDelta
//! [`ClusterClientRegistry`]: crate::client::cluster::ClusterClientRegistry

use std::sync::Arc;

use tonic::transport::{Channel, Endpoint};

use crate::client::endpoint::{Connector, EndpointAddress, EndpointChannel};
use crate::client::lb::{ClusterDiscovery, EndpointStream};
use crate::common::async_util::BoxFuture;
use crate::xds::cache::XdsCache;
use crate::xds::cert_provider::CertProviderRegistry;
use crate::xds::endpoint_manager::discover_endpoint_deltas;
use crate::xds::resource::ClusterResource;

/// xDS-backed cluster discovery — pure EDS subscription.
pub(crate) struct XdsClusterDiscovery {
    cache: Arc<XdsCache>,
}

impl XdsClusterDiscovery {
    pub(crate) fn new(cache: Arc<XdsCache>) -> Self {
        Self { cache }
    }
}

impl ClusterDiscovery for XdsClusterDiscovery {
    fn discover_cluster(&self, cluster_name: &str) -> EndpointStream {
        discover_endpoint_deltas(self.cache.watch_endpoints(cluster_name))
    }
}

/// Build a [`Connector`] for a given cluster based on its security config.
///
/// Returns a [`PlaintextConnector`] when the cluster has no `transport_socket`.
/// When TLS is required, returns [`ConnectorBuildError::TlsNotWired`] —
/// rustls-side parsing/validation (see [`crate::xds::cert_provider::verifier`])
/// is in place, but plumbing a custom [`rustls::client::danger::ServerCertVerifier`]
/// into a `tonic::transport::Channel` requires
/// `ClientTlsConfig::with_server_cert_verifier`, which does not yet exist
/// upstream.
pub(crate) fn build_connector(
    cluster: &ClusterResource,
    _registry: &Arc<CertProviderRegistry>,
) -> Result<Arc<dyn Connector<Service = EndpointChannel<Channel>> + Send + Sync>, ConnectorBuildError>
{
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
