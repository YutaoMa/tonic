//! Configuration for the XDS client.
use std::time::Duration;

/// Configuration for the TLS connection.
/// This is used to configure the TLS connection to the XDS server.
#[derive(Clone, Debug, Default)]
pub struct TlsConfig {
    /// The CA certificate to use for the TLS connection.
    pub ca_cert_pem: Option<Vec<u8>>,
    /// The client certificate to use for the TLS connection.
    pub client_cert_pem: Option<Vec<u8>>,
    /// The client key to use for the TLS connection.
    pub client_key_pem: Option<Vec<u8>>,
    /// The domain name to use for the TLS connection.
    pub domain_name: Option<String>,
}

/// Configuration for the XDS client.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// The URI of the XDS server to connect to.
    pub server_uri: String,
    /// The node ID to use for the XDS connection.
    pub node_id: String,
    /// The timeout to use for the XDS connection.
    pub connect_timeout: Duration,
    /// The TLS configuration to use for the XDS connection.
    pub tls_config: Option<TlsConfig>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_uri: "http://localhost:50005".to_string(),
            node_id: "grpc".to_string(),
            connect_timeout: Duration::from_secs(5),
            tls_config: None,
        }
    }
}
