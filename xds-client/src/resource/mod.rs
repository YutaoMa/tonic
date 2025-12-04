//! Resources for XDS.
use crate::error::Result;

pub mod listener;
pub mod route;

pub use listener::ListenerResource;
pub use route::RouteResource;

/// Trait for XDS resources.
pub trait XdsResource: Send + Sync + std::fmt::Debug + 'static {
    /// The resource type.
    type Resource: Send + Sync + Clone + std::fmt::Debug + 'static;

    /// The type URL of the resource.
    fn type_url() -> &'static str;

    /// Decode a resource from a byte array.
    fn decode(data: &[u8]) -> Result<Self::Resource>;
}
