//! Transport for the XDS client.
use bytes::Bytes;
use futures::{Sink, Stream};
use crate::error::{Error, Result};
use crate::pb::envoy::service::discovery::v3::{DiscoveryRequest, DiscoveryResponse};
use crate::pb::envoy::config::core::v3::Node;

pub mod tonic;

/// Discovery request for the XDS client.
#[derive(Debug)]
pub struct XdsDiscoveryRequest {
    /// The version info of the request.
    pub version_info: String,
    /// The node ID of the request.
    pub node_id: String,
    /// The resource names of the request.
    pub resource_names: Vec<String>,
    /// The type URL of the request.
    pub type_url: String,
    /// The response nonce of the request.
    pub response_nonce: String,
    /// The error detail of the request.
    pub error_detail: Option<String>,
}

/// Discovery response for the XDS client.
#[derive(Debug)]
pub struct XdsDiscoveryResponse {
    /// The type URL of the response.
    pub type_url: String,
    /// The version info of the response.
    pub version_info: String,
    /// The nonce of the response.
    pub nonce: String,
    /// The resources of the response.
    pub resources: Vec<Bytes>,
}

impl From<XdsDiscoveryRequest> for DiscoveryRequest {
    fn from(req: XdsDiscoveryRequest) -> Self {
        DiscoveryRequest {
            version_info: req.version_info,
            node: Some(Node {
                id: req.node_id,
                user_agent_name: "grpc".to_string(),
                client_features: vec!["xds.v3".to_string()],
                ..Default::default()
            }),
            resource_names: req.resource_names,
            type_url: req.type_url,
            response_nonce: req.response_nonce,
            error_detail: None,
            resource_locators: vec![],
        }
    }
}

impl TryFrom<DiscoveryResponse> for XdsDiscoveryResponse {
    type Error = Error;

    fn try_from(resp: DiscoveryResponse) -> Result<Self> {
        Ok(XdsDiscoveryResponse {
            type_url: resp.type_url,
            version_info: resp.version_info,
            nonce: resp.nonce,
            resources: resp.resources.into_iter().map(|any| Bytes::from(any.value)).collect(),
        })
    }
}

#[async_trait::async_trait]
/// Trait for transport factories.
pub trait TransportFactory: Send + Sync + std::fmt::Debug + 'static {
    /// The type of the stream.
    type Stream: XdsStream;

    /// Create a new stream.
    async fn create_stream(&self) -> Result<Self::Stream>;
}

/// Trait for XDS streams.
pub trait XdsStream:
    Stream<Item = Result<XdsDiscoveryResponse>>
    + Sink<XdsDiscoveryRequest, Error = Error>
    + Send
    + Unpin
{}

impl<T> XdsStream for T where
    T: Stream<Item = Result<XdsDiscoveryResponse>>
        + Sink<XdsDiscoveryRequest, Error = Error>
        + Send
        + Unpin
{}
