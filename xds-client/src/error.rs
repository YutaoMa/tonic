//! Error types for the XDS client.
use thiserror::Error;

/// Error type for the XDS client.
#[derive(Debug, Error)]
pub enum Error {
    /// Transport error.
    #[error("Transport error: {0}")]
    Transport(#[source] Box<dyn std::error::Error + Send + Sync>),
    
    /// Tonic transport error.
    #[error("Tonic transport error: {0}")]
    TonicTransport(#[from] tonic::transport::Error),

    /// gRPC status error.
    #[error("gRPC status error: {0}")]
    GrpcStatus(#[from] tonic::Status),

    /// Resource decode error.
    #[error("Resource decode error: {0}")]
    Decode(#[from] prost::DecodeError),

    /// Watch error.
    #[error("Watch error: {0}")]
    Watch(String),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),
    
    /// Field missing error.
    #[error("Field missing: {0}")]
    FieldMissing(String),
}

/// Result type for the XDS client.
pub type Result<T> = std::result::Result<T, Error>;
