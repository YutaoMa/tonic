/*
 *
 * Copyright 2025 gRPC authors.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to
 * deal in the Software without restriction, including without limitation the
 * rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
 * sell copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
 * FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
 * IN THE SOFTWARE.
 *
 */

//! xDS bootstrap configuration.
//!
//! Parses the bootstrap JSON from `GRPC_XDS_BOOTSTRAP` (file path) or
//! `GRPC_XDS_BOOTSTRAP_CONFIG` (inline JSON) environment variables,
//! per gRFC A27.

use std::collections::HashMap;

use serde::Deserialize;
use xds_client::message::{Locality, MetadataValue, Node};

/// Environment variable pointing to a bootstrap JSON file path.
const ENV_BOOTSTRAP_FILE: &str = "GRPC_XDS_BOOTSTRAP";
/// Environment variable containing inline bootstrap JSON.
const ENV_BOOTSTRAP_CONFIG: &str = "GRPC_XDS_BOOTSTRAP_CONFIG";

/// Parsed xDS bootstrap configuration per [gRFC A27].
///
/// The bootstrap tells the xDS client where the management server lives
/// and what identity (node) to present. It is typically loaded from a
/// JSON file or environment variable.
///
/// # Loading
///
/// ```rust,no_run
/// use tonic_xds::BootstrapConfig;
///
/// // From environment variable (GRPC_XDS_BOOTSTRAP or GRPC_XDS_BOOTSTRAP_CONFIG):
/// let config = BootstrapConfig::from_env().unwrap();
///
/// // From a JSON string:
/// let json = r#"{"xds_servers":[{"server_uri":"xds.example.com:443"}]}"#;
/// let config = BootstrapConfig::from_json(json).unwrap();
/// ```
///
/// # Inspecting
///
/// The fields are private so new bootstrap keys can be added without breaking
/// changes. The accessors below report what a loaded config will act on.
///
/// ```rust
/// use tonic_xds::BootstrapConfig;
///
/// let json = r#"{
///   "xds_servers": [{"server_uri": "xds.example.com:443"}],
///   "node": {"id": "node-1"}
/// }"#;
/// let config = BootstrapConfig::from_json(json).unwrap();
/// assert_eq!(config.server_uri(), "xds.example.com:443");
/// assert_eq!(config.node_id(), "node-1");
/// ```
///
/// # Building programmatically
///
/// [`BootstrapConfig::builder`] constructs a config from typed values,
/// without needing a JSON string. It covers the
/// same bootstrap keys [`from_json`] acts on, per gRFC A27.
///
/// ```rust
/// use tonic_xds::{BootstrapConfig, ChannelCredentialType};
///
/// let config = BootstrapConfig::builder("xds.example.com:443")
///     .channel_creds([ChannelCredentialType::Tls])
///     .node_id("node-1")
///     .build()
///     .unwrap();
///
/// assert_eq!(config.server_uri(), "xds.example.com:443");
/// assert!(config.use_tls());
/// ```
///
/// [`from_env`]: BootstrapConfig::from_env
/// [`from_json`]: BootstrapConfig::from_json
/// [gRFC A27]: https://github.com/grpc/proposal/blob/master/A27-xds-global-load-balancing.md
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "BootstrapConfigDe")]
#[non_exhaustive]
pub struct BootstrapConfig {
    /// xDS management servers to connect to.
    pub(crate) xds_servers: Vec<XdsServerConfig>,
    /// Node identity sent to the xDS server.
    pub(crate) node: NodeConfig,
    /// Certificate provider plugin instances, keyed by instance name.
    ///
    /// Referenced by [`CertificateProviderPluginInstance`] in CDS/LDS
    /// `UpstreamTlsContext` / `DownstreamTlsContext` resources.
    /// See gRFC A29 for details.
    ///
    /// [`CertificateProviderPluginInstance`]: https://github.com/envoyproxy/envoy/blob/main/api/envoy/extensions/transport_sockets/tls/v3/common.proto
    // Consumed by `CertProviderRegistry::from_bootstrap` only under TLS
    // features; parsed regardless so non-TLS builds accept the same JSON.
    #[cfg_attr(not(feature = "_tls-any"), allow(dead_code))]
    pub(crate) certificate_providers: HashMap<String, CertProviderPluginConfig>,
}

/// Wire form of [`BootstrapConfig`].
///
/// [`BootstrapConfig`] deserializes through this type via `serde(try_from)`,
/// so a config a caller obtains by embedding it in their own config struct
/// goes through [`validate`](BootstrapConfig::validate) too.
#[derive(Deserialize)]
pub(crate) struct BootstrapConfigDe {
    xds_servers: Vec<XdsServerConfig>,
    #[serde(default)]
    node: NodeConfig,
    #[serde(default)]
    certificate_providers: HashMap<String, CertProviderPluginConfig>,
}

impl TryFrom<BootstrapConfigDe> for BootstrapConfig {
    type Error = BootstrapError;

    fn try_from(de: BootstrapConfigDe) -> Result<Self, Self::Error> {
        let config = Self {
            xds_servers: de.xds_servers,
            node: de.node,
            certificate_providers: de.certificate_providers,
        };
        config.validate()?;
        Ok(config)
    }
}

/// Configuration for a single xDS management server.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct XdsServerConfig {
    /// URI of the xDS server (e.g., `"xds.example.com:443"`).
    pub server_uri: String,
    /// Ordered list of channel credentials. The client uses the first supported type.
    #[serde(default)]
    pub channel_creds: Vec<ChannelCredentialConfig>,
    /// Server features (e.g., `["xds_v3"]`).
    #[serde(default)]
    #[allow(dead_code)]
    // Parsed for completeness; used when server feature negotiation is added.
    pub server_features: Vec<String>,
}

/// A channel credential entry from the bootstrap config.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct ChannelCredentialConfig {
    /// Credential type (e.g., `"insecure"`, `"tls"`, `"google_default"`).
    #[serde(rename = "type")]
    pub cred_type: ChannelCredentialType,
}

/// Channel credential type offered to the xDS management server.
///
/// The client uses the first type it supports, per [gRFC A27]. This is the
/// bootstrap's `xds_servers[].channel_creds[].type`, and is what
/// [`BootstrapConfigBuilder::channel_creds`] accepts.
///
/// [gRFC A27]: https://github.com/grpc/proposal/blob/master/A27-xds-global-load-balancing.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ChannelCredentialType {
    /// Plaintext connection to the xDS server.
    Insecure,
    /// TLS with the platform's default trust roots.
    Tls,
    /// Google default credentials (ALTS or TLS plus call credentials).
    GoogleDefault,
    /// A credential type this client does not implement.
    ///
    /// Produced when parsing a bootstrap written for a newer or different
    /// client; such entries are skipped when selecting a credential, so a
    /// bootstrap can list types this version does not know about.
    ///
    /// `#[non_exhaustive]` reserves the variant for the parser: downstream
    /// code can neither construct it nor match its payload. [`Deserialize`]
    /// still yields one for an unknown type, so
    /// [`BootstrapConfigBuilder::build`] rejects it.
    #[serde(untagged)]
    #[non_exhaustive]
    Unsupported(String),
}

/// A certificate provider plugin entry from the bootstrap config.
///
/// Holds the `plugin_name` and an opaque `config` blob. The cert provider
/// module is responsible for dispatching on `plugin_name` and deserializing
/// `config` into the appropriate plugin-specific type.
///
/// Referenced by `instance_name` in CDS/LDS `CertificateProviderPluginInstance`
/// fields. See [gRFC A29].
///
/// [gRFC A29]: https://github.com/grpc/proposal/blob/master/A29-xds-tls-security.md
#[derive(Debug, Clone, PartialEq, Deserialize)]
// In non-TLS builds `cert_provider` is gated out, so nothing reads these
// fields after serde populates them.
#[cfg_attr(not(feature = "_tls-any"), allow(dead_code))]
pub(crate) struct CertProviderPluginConfig {
    pub plugin_name: String,
    #[serde(default)]
    pub config: serde_json::Value,
}

/// Node identity configuration from bootstrap JSON.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub(crate) struct NodeConfig {
    /// Opaque node identifier.
    #[serde(default)]
    pub id: String,
    /// Cluster the node belongs to.
    pub cluster: Option<String>,
    /// Locality where the node is running.
    pub locality: Option<LocalityConfig>,
    /// Free-form metadata sent to the xDS server (`google.protobuf.Struct`).
    ///
    /// Accepts any JSON value (nested objects, arrays, numbers, bools, null)
    /// per the proto3 JSON mapping for `google.protobuf.Struct`. Some control
    /// planes vary the served config based on metadata — e.g. Istio's istiod
    /// gates proxyless gRPC config behind `GENERATOR = "grpc"`.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Convert a `serde_json::Value` to the codec-agnostic [`MetadataValue`].
fn json_to_metadata(value: serde_json::Value) -> Result<MetadataValue, BootstrapError> {
    Ok(match value {
        serde_json::Value::Null => MetadataValue::Null,
        serde_json::Value::Bool(b) => MetadataValue::Bool(b),
        serde_json::Value::Number(n) => MetadataValue::Number(n.as_f64().ok_or_else(|| {
            BootstrapError::Validation(format!("metadata number {n} not representable as f64"))
        })?),
        serde_json::Value::String(s) => MetadataValue::String(s),
        serde_json::Value::Array(arr) => MetadataValue::Array(
            arr.into_iter()
                .map(json_to_metadata)
                .collect::<Result<_, _>>()?,
        ),
        serde_json::Value::Object(obj) => MetadataValue::Object(
            obj.into_iter()
                .map(|(k, v)| json_to_metadata(v).map(|mv| (k, mv)))
                .collect::<Result<_, _>>()?,
        ),
    })
}

/// Locality configuration from bootstrap JSON.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct LocalityConfig {
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub zone: String,
    #[serde(default)]
    pub sub_zone: String,
}

/// Errors that can occur when loading bootstrap configuration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BootstrapError {
    /// Neither `GRPC_XDS_BOOTSTRAP` nor `GRPC_XDS_BOOTSTRAP_CONFIG` is set.
    #[error("neither {ENV_BOOTSTRAP_FILE} nor {ENV_BOOTSTRAP_CONFIG} environment variable is set")]
    NotConfigured,
    /// Failed to read the bootstrap JSON file.
    #[error("failed to read bootstrap file '{path}': {source}")]
    ReadFile {
        /// Path that could not be read.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The JSON could not be parsed.
    #[error("failed to parse bootstrap JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    /// The parsed config failed validation (e.g., empty `xds_servers`).
    #[error("bootstrap config validation failed: {0}")]
    Validation(String),
    /// A value passed to [`BootstrapConfigBuilder`] could not be serialized to
    /// JSON.
    ///
    /// Setters take `impl Serialize` for free-form fields such as
    /// `node.metadata`; [`build`](BootstrapConfigBuilder::build) reports the
    /// first one that failed.
    #[error("failed to serialize bootstrap {field}: {source}")]
    Serialization {
        /// Field whose value could not be serialized (e.g. `node.metadata`).
        field: &'static str,
        /// Underlying serialization error.
        source: serde_json::Error,
    },
}

impl BootstrapConfig {
    /// Load bootstrap configuration from environment variables.
    ///
    /// Checks `GRPC_XDS_BOOTSTRAP` (file path) first, then falls back to
    /// `GRPC_XDS_BOOTSTRAP_CONFIG` (inline JSON).
    pub fn from_env() -> Result<Self, BootstrapError> {
        if let Ok(path) = std::env::var(ENV_BOOTSTRAP_FILE) {
            let json = std::fs::read_to_string(&path)
                .map_err(|e| BootstrapError::ReadFile { path, source: e })?;
            return Self::from_json(&json);
        }

        if let Ok(json) = std::env::var(ENV_BOOTSTRAP_CONFIG) {
            return Self::from_json(&json);
        }

        Err(BootstrapError::NotConfigured)
    }

    /// Parse bootstrap configuration from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, BootstrapError> {
        // The wire form reports validation failures as
        // `BootstrapError::Validation`; `Self` reports them as a serde error.
        let de: BootstrapConfigDe = serde_json::from_str(json)?;
        Self::try_from(de)
    }

    /// Checks the invariants every constructor upholds.
    ///
    /// Runs on both construction paths — `TryFrom<BootstrapConfigDe>`, which
    /// covers [`from_json`], [`from_env`] and `Deserialize`, and
    /// [`BootstrapConfigBuilder::build`] — so [`server_uri`] can index
    /// `xds_servers` directly.
    ///
    /// [`from_json`]: BootstrapConfig::from_json
    /// [`from_env`]: BootstrapConfig::from_env
    /// [`server_uri`]: BootstrapConfig::server_uri
    fn validate(&self) -> Result<(), BootstrapError> {
        if self.xds_servers.is_empty() {
            return Err(BootstrapError::Validation(
                "xds_servers must not be empty".into(),
            ));
        }
        for (i, server) in self.xds_servers.iter().enumerate() {
            if server.server_uri.is_empty() {
                return Err(BootstrapError::Validation(format!(
                    "xds_servers[{i}].server_uri must not be empty"
                )));
            }
        }
        Ok(())
    }

    /// Returns the URI of the xDS server this config connects to.
    ///
    /// Only the first entry is used; further `xds_servers` are parsed but not
    /// yet connected to.
    pub fn server_uri(&self) -> &str {
        self.xds_servers
            .first()
            .map(|s| s.server_uri.as_str())
            .expect("xds_servers validated non-empty")
    }

    /// Returns the node identifier presented to the xDS server.
    ///
    /// Empty when the bootstrap omits `node.id`.
    pub fn node_id(&self) -> &str {
        &self.node.id
    }

    /// Select the first supported channel credential type from the first server's config.
    ///
    /// Per gRFC A27, the client stops at the first credential type it supports.
    /// Returns `None` if no supported credential type is found.
    pub(crate) fn selected_credential(&self) -> Option<&ChannelCredentialType> {
        self.xds_servers
            .first()?
            .channel_creds
            .iter()
            .map(|c| &c.cred_type)
            .find(|t| {
                matches!(
                    t,
                    ChannelCredentialType::Insecure
                        | ChannelCredentialType::Tls
                        | ChannelCredentialType::GoogleDefault
                )
            })
    }

    /// Returns `true` if the connection to the xDS server uses TLS.
    pub fn use_tls(&self) -> bool {
        matches!(
            self.selected_credential(),
            Some(ChannelCredentialType::Tls | ChannelCredentialType::GoogleDefault)
        )
    }

    /// Starts building a bootstrap config for the given xDS server URI.
    ///
    /// Use this when the process already holds the configuration, so it can
    /// build the config directly. [`from_json`] remains the full-fidelity
    /// path and the only one that accepts a gRFC A27 document verbatim.
    ///
    /// ```rust
    /// use tonic_xds::{BootstrapConfig, ChannelCredentialType};
    ///
    /// let config = BootstrapConfig::builder("xds.example.com:443")
    ///     .channel_creds([ChannelCredentialType::Tls])
    ///     .node_id("my-node")
    ///     .build()
    ///     .unwrap();
    ///
    /// assert_eq!(config.server_uri(), "xds.example.com:443");
    /// assert_eq!(config.node_id(), "my-node");
    /// assert!(config.use_tls());
    /// ```
    ///
    /// [`from_json`]: BootstrapConfig::from_json
    pub fn builder(server_uri: impl Into<String>) -> BootstrapConfigBuilder {
        BootstrapConfigBuilder::new(server_uri)
    }
}

/// Builds a [`BootstrapConfig`] without going through JSON.
///
/// Created by [`BootstrapConfig::builder`]. Every setter is optional except
/// the server URI, which [`BootstrapConfig::builder`] takes up front because
/// the config is invalid without it.
///
/// The builder covers the bootstrap keys the client acts on. A bootstrap that
/// needs keys outside that set — additional `xds_servers` entries, which are
/// parsed but not yet connected to — must still come from [`from_json`].
///
/// ```rust
/// use tonic_xds::{BootstrapConfig, ChannelCredentialType};
///
/// let config = BootstrapConfig::builder("xds.example.com:443")
///     .channel_creds([ChannelCredentialType::Tls, ChannelCredentialType::Insecure])
///     .node_id("projects/123/nodes/456")
///     .node_cluster("my-cluster")
///     .node_locality("us-east1", "us-east1-b", "rack1")
///     .node_metadata("GENERATOR", "grpc")
///     .build()
///     .unwrap();
///
/// assert!(config.use_tls());
/// ```
///
/// [`from_json`]: BootstrapConfig::from_json
#[derive(Debug)]
pub struct BootstrapConfigBuilder {
    server_uri: String,
    channel_creds: Vec<ChannelCredentialConfig>,
    node_id: String,
    node_cluster: Option<String>,
    node_locality: Option<LocalityConfig>,
    node_metadata: HashMap<String, serde_json::Value>,
    certificate_providers: HashMap<String, CertProviderPluginConfig>,
    error: Option<BootstrapError>,
}

impl BootstrapConfigBuilder {
    fn new(server_uri: impl Into<String>) -> Self {
        Self {
            server_uri: server_uri.into(),
            channel_creds: Vec::new(),
            node_id: String::new(),
            node_cluster: None,
            node_locality: None,
            node_metadata: HashMap::new(),
            certificate_providers: HashMap::new(),
            error: None,
        }
    }

    /// Sets the credentials offered to the xDS server, in preference order.
    ///
    /// Replaces any previously set credentials. Unset, the client connects to
    /// the management server in plaintext, matching a bootstrap with no
    /// `channel_creds`; pass [`ChannelCredentialType::Tls`] for a secured
    /// server.
    #[must_use]
    pub fn channel_creds(mut self, creds: impl IntoIterator<Item = ChannelCredentialType>) -> Self {
        self.channel_creds = creds
            .into_iter()
            .map(|cred_type| ChannelCredentialConfig { cred_type })
            .collect();
        self
    }

    /// Sets the opaque node identifier presented to the xDS server.
    #[must_use]
    pub fn node_id(mut self, id: impl Into<String>) -> Self {
        self.node_id = id.into();
        self
    }

    /// Sets the cluster the node belongs to.
    #[must_use]
    pub fn node_cluster(mut self, cluster: impl Into<String>) -> Self {
        self.node_cluster = Some(cluster.into());
        self
    }

    /// Sets the locality the node is running in.
    #[must_use]
    pub fn node_locality(
        mut self,
        region: impl Into<String>,
        zone: impl Into<String>,
        sub_zone: impl Into<String>,
    ) -> Self {
        self.node_locality = Some(LocalityConfig {
            region: region.into(),
            zone: zone.into(),
            sub_zone: sub_zone.into(),
        });
        self
    }

    /// Adds one free-form node metadata entry, replacing any entry with the
    /// same key.
    ///
    /// Accepts anything [`Serialize`], so a string, a number, or a nested
    /// struct all work. Some control planes vary the served config based on
    /// metadata — e.g. Istio's istiod gates proxyless gRPC config behind
    /// `GENERATOR = "grpc"`.
    ///
    /// [`build`] reports a value with no JSON form as
    /// [`BootstrapError::Serialization`].
    ///
    /// [`Serialize`]: serde::Serialize
    /// [`build`]: BootstrapConfigBuilder::build
    #[must_use]
    pub fn node_metadata(mut self, key: impl Into<String>, value: impl serde::Serialize) -> Self {
        let key = key.into();
        match serde_json::to_value(value) {
            Ok(value) => {
                self.node_metadata.insert(key, value);
            }
            Err(source) => self.record_error("node.metadata", source),
        }
        self
    }

    /// Registers a certificate provider instance, replacing any instance with
    /// the same name.
    ///
    /// `instance_name` is what CDS/LDS resources reference by
    /// `CertificateProviderPluginInstance`; `config` is the opaque
    /// plugin-specific blob, accepted as anything [`Serialize`]. See [gRFC A29].
    ///
    /// [`build`] reports a config with no JSON form as
    /// [`BootstrapError::Serialization`].
    ///
    /// [`Serialize`]: serde::Serialize
    /// [`build`]: BootstrapConfigBuilder::build
    /// [gRFC A29]: https://github.com/grpc/proposal/blob/master/A29-xds-tls-security.md
    #[must_use]
    pub fn certificate_provider(
        mut self,
        instance_name: impl Into<String>,
        plugin_name: impl Into<String>,
        config: impl serde::Serialize,
    ) -> Self {
        let instance_name = instance_name.into();
        match serde_json::to_value(config) {
            Ok(config) => {
                self.certificate_providers.insert(
                    instance_name,
                    CertProviderPluginConfig {
                        plugin_name: plugin_name.into(),
                        config,
                    },
                );
            }
            Err(source) => self.record_error("certificate_providers", source),
        }
        self
    }

    /// Keeps the first error so the chain can continue and report at `build`.
    fn record_error(&mut self, field: &'static str, source: serde_json::Error) {
        if self.error.is_none() {
            self.error = Some(BootstrapError::Serialization { field, source });
        }
    }

    /// Validates the accumulated settings and returns the config.
    ///
    /// # Errors
    ///
    /// - [`BootstrapError::Serialization`] if a metadata or certificate
    ///   provider value could not be serialized to JSON; the first failing
    ///   setter wins.
    /// - [`BootstrapError::Validation`] if the server URI is empty — the same
    ///   validation configs parsed from JSON go through — or if the
    ///   credentials include [`ChannelCredentialType::Unsupported`].
    pub fn build(self) -> Result<BootstrapConfig, BootstrapError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        // Parsing skips `Unsupported` for forward compatibility; building
        // treats it as a caller mistake. `Deserialize` is the only source of
        // one, since the constructor is sealed.
        if let Some(ChannelCredentialType::Unsupported(name)) = self
            .channel_creds
            .iter()
            .map(|c| &c.cred_type)
            .find(|t| matches!(t, ChannelCredentialType::Unsupported(_)))
        {
            return Err(BootstrapError::Validation(format!(
                "channel credential type '{name}' is not supported by this client"
            )));
        }
        let config = BootstrapConfig {
            xds_servers: vec![XdsServerConfig {
                server_uri: self.server_uri,
                channel_creds: self.channel_creds,
                // The client acts on no server feature yet, so the builder
                // omits a setter.
                server_features: Vec::new(),
            }],
            node: NodeConfig {
                id: self.node_id,
                cluster: self.node_cluster,
                locality: self.node_locality,
                metadata: self.node_metadata,
            },
            certificate_providers: self.certificate_providers,
        };
        config.validate()?;
        Ok(config)
    }
}

impl TryFrom<NodeConfig> for Node {
    type Error = BootstrapError;

    fn try_from(config: NodeConfig) -> Result<Self, Self::Error> {
        let mut node = Node::new("tonic-xds", env!("CARGO_PKG_VERSION"));

        if !config.id.is_empty() {
            node = node.with_id(config.id);
        }
        if let Some(cluster) = config.cluster {
            node = node.with_cluster(cluster);
        }
        if let Some(locality) = config.locality {
            node = node.with_locality(Locality {
                region: locality.region,
                zone: locality.zone,
                sub_zone: locality.sub_zone,
            });
        }
        if !config.metadata.is_empty() {
            let metadata: HashMap<String, MetadataValue> = config
                .metadata
                .into_iter()
                .map(|(k, v)| json_to_metadata(v).map(|mv| (k, mv)))
                .collect::<Result<_, _>>()?;
            node = node.with_metadata(metadata);
        }

        Ok(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_json() -> &'static str {
        r#"{
            "xds_servers": [{"server_uri": "xds.example.com:443"}],
            "node": {"id": "test-node"}
        }"#
    }

    fn full_json() -> &'static str {
        r#"{
            "xds_servers": [{
                "server_uri": "xds.example.com:443",
                "channel_creds": [
                    {"type": "google_default"},
                    {"type": "tls"},
                    {"type": "insecure"}
                ],
                "server_features": ["xds_v3"]
            }],
            "node": {
                "id": "projects/123/nodes/456",
                "cluster": "test-cluster",
                "locality": {
                    "region": "us-east1",
                    "zone": "us-east1-b",
                    "sub_zone": "rack1"
                }
            }
        }"#
    }

    #[test]
    fn parse_minimal() {
        let config = BootstrapConfig::from_json(minimal_json()).unwrap();
        assert_eq!(config.xds_servers.len(), 1);
        assert_eq!(config.server_uri(), "xds.example.com:443");
        assert_eq!(config.node.id, "test-node");
        assert!(config.node.cluster.is_none());
        assert!(config.node.locality.is_none());
    }

    #[test]
    fn parse_full() {
        let config = BootstrapConfig::from_json(full_json()).unwrap();
        assert_eq!(config.xds_servers[0].server_uri, "xds.example.com:443");
        assert_eq!(config.xds_servers[0].channel_creds.len(), 3);
        assert!(matches!(
            config.xds_servers[0].channel_creds[0].cred_type,
            ChannelCredentialType::GoogleDefault
        ));
        assert_eq!(config.xds_servers[0].server_features, vec!["xds_v3"]);
        assert_eq!(config.node.id, "projects/123/nodes/456");
        assert_eq!(config.node.cluster.as_deref(), Some("test-cluster"));

        let locality = config.node.locality.as_ref().unwrap();
        assert_eq!(locality.region, "us-east1");
        assert_eq!(locality.zone, "us-east1-b");
        assert_eq!(locality.sub_zone, "rack1");
    }

    #[test]
    fn node_from_full_config() {
        let config = BootstrapConfig::from_json(full_json()).unwrap();
        let node = Node::try_from(config.node).unwrap();
        assert_eq!(node.id.as_deref(), Some("projects/123/nodes/456"));
        assert_eq!(node.cluster.as_deref(), Some("test-cluster"));
        assert_eq!(node.user_agent_name, "tonic-xds");

        let locality = node.locality.unwrap();
        assert_eq!(locality.region, "us-east1");
        assert_eq!(locality.zone, "us-east1-b");
        assert_eq!(locality.sub_zone, "rack1");
    }

    #[test]
    fn node_from_minimal_config() {
        let config = BootstrapConfig::from_json(minimal_json()).unwrap();
        let node = Node::try_from(config.node).unwrap();
        assert_eq!(node.id.as_deref(), Some("test-node"));
        assert!(node.cluster.is_none());
        assert!(node.locality.is_none());
    }

    #[test]
    fn selected_credential_first_supported_wins() {
        let config = BootstrapConfig::from_json(full_json()).unwrap();
        assert_eq!(
            config.selected_credential(),
            Some(&ChannelCredentialType::GoogleDefault)
        );
    }

    #[test]
    fn selected_credential_insecure() {
        let json = r#"{
            "xds_servers": [{
                "server_uri": "localhost:5000",
                "channel_creds": [{"type": "insecure"}]
            }],
            "node": {"id": "n1"}
        }"#;
        let config = BootstrapConfig::from_json(json).unwrap();
        assert_eq!(
            config.selected_credential(),
            Some(&ChannelCredentialType::Insecure)
        );
    }

    #[test]
    fn selected_credential_none_supported() {
        let json = r#"{
            "xds_servers": [{
                "server_uri": "localhost:5000",
                "channel_creds": [{"type": "some_future_type"}]
            }],
            "node": {"id": "n1"}
        }"#;
        let config = BootstrapConfig::from_json(json).unwrap();
        assert!(matches!(
            config.xds_servers[0].channel_creds[0].cred_type,
            ChannelCredentialType::Unsupported(_)
        ));
        assert_eq!(config.selected_credential(), None);
    }

    #[test]
    fn selected_credential_empty_creds() {
        let config = BootstrapConfig::from_json(minimal_json()).unwrap();
        assert_eq!(config.selected_credential(), None);
    }

    #[test]
    fn empty_xds_servers_fails() {
        let json = r#"{"xds_servers": [], "node": {"id": "n1"}}"#;
        let err = BootstrapConfig::from_json(json).unwrap_err();
        assert!(err.to_string().contains("xds_servers must not be empty"));
    }

    #[test]
    fn empty_server_uri_fails() {
        let json = r#"{"xds_servers": [{"server_uri": ""}], "node": {"id": "n1"}}"#;
        let err = BootstrapConfig::from_json(json).unwrap_err();
        assert!(err.to_string().contains("server_uri must not be empty"));
    }

    #[test]
    fn invalid_json_fails() {
        let err = BootstrapConfig::from_json("not json").unwrap_err();
        assert!(matches!(err, BootstrapError::InvalidJson(_)));
    }

    #[test]
    fn missing_required_field_fails() {
        let json = r#"{"node": {"id": "n1"}}"#;
        let err = BootstrapConfig::from_json(json).unwrap_err();
        assert!(err.to_string().contains("xds_servers"));
    }

    #[test]
    fn node_without_id() {
        let json = r#"{
            "xds_servers": [{"server_uri": "localhost:5000"}]
        }"#;
        let config = BootstrapConfig::from_json(json).unwrap();
        let node = Node::try_from(config.node).unwrap();
        assert!(node.id.is_none());
    }

    #[test]
    fn public_accessors_report_what_the_client_will_use() {
        let json = r#"{
            "xds_servers": [
                {"server_uri": "primary:443", "channel_creds": [{"type": "tls"}]},
                {"server_uri": "fallback:443"}
            ],
            "node": {"id": "node-1"}
        }"#;
        let config = BootstrapConfig::from_json(json).unwrap();

        assert_eq!(config.server_uri(), "primary:443");
        assert_eq!(config.node_id(), "node-1");
        assert!(config.use_tls());
    }

    #[test]
    fn public_accessors_report_absent_optional_fields() {
        let json = r#"{"xds_servers": [{"server_uri": "localhost:5000"}]}"#;
        let config = BootstrapConfig::from_json(json).unwrap();

        assert_eq!(config.node_id(), "");
        assert!(!config.use_tls());
    }

    #[test]
    fn equal_configs_compare_equal_regardless_of_json_formatting() {
        let compact = r#"{"xds_servers":[{"server_uri":"xds:443"}],"node":{"id":"n1"}}"#;
        let spaced = r#"{
            "node": {"id": "n1"},
            "xds_servers": [{"server_uri": "xds:443"}]
        }"#;

        assert_eq!(
            BootstrapConfig::from_json(compact).unwrap(),
            BootstrapConfig::from_json(spaced).unwrap(),
        );
    }

    #[test]
    fn misplaced_keys_are_dropped_and_compare_unequal() {
        let intended = r#"{"xds_servers":[{"server_uri":"xds:443"}],"node":{"id":"n1"}}"#;
        let misplaced = r#"{"xds_servers":[{"server_uri":"xds:443"}],"node_id":"n1"}"#;

        let misparsed = BootstrapConfig::from_json(misplaced).unwrap();
        assert_eq!(misparsed.node_id(), "");
        assert_ne!(BootstrapConfig::from_json(intended).unwrap(), misparsed);
    }

    #[test]
    fn builder_matches_the_equivalent_json() {
        let built = BootstrapConfig::builder("xds.example.com:443")
            .channel_creds([
                ChannelCredentialType::GoogleDefault,
                ChannelCredentialType::Tls,
                ChannelCredentialType::Insecure,
            ])
            .node_id("projects/123/nodes/456")
            .node_cluster("test-cluster")
            .node_locality("us-east1", "us-east1-b", "rack1")
            .build()
            .unwrap();

        // `full_json` also carries `server_features`; the client ignores it,
        // so the builder omits it.
        let parsed = BootstrapConfig::from_json(full_json()).unwrap();
        assert_eq!(parsed.xds_servers[0].server_features, vec!["xds_v3"]);
        assert!(built.xds_servers[0].server_features.is_empty());

        let mut parsed_without_features = parsed;
        parsed_without_features.xds_servers[0]
            .server_features
            .clear();
        assert_eq!(parsed_without_features, built);
    }

    #[test]
    fn builder_defaults_omit_optional_fields() {
        let built = BootstrapConfig::builder("xds.example.com:443")
            .node_id("test-node")
            .build()
            .unwrap();

        assert_eq!(BootstrapConfig::from_json(minimal_json()).unwrap(), built);
        assert!(!built.use_tls());
    }

    #[test]
    fn builder_carries_metadata_and_cert_providers() {
        let built = BootstrapConfig::builder("localhost:5000")
            .node_metadata("GENERATOR", "grpc")
            .certificate_provider(
                "google_cloud_private_spiffe",
                "file_watcher",
                serde_json::json!({"certificate_file": "/etc/certs/cert.pem"}),
            )
            .build()
            .unwrap();

        let equivalent = BootstrapConfig::from_json(
            r#"{
                "xds_servers": [{"server_uri": "localhost:5000"}],
                "node": {"metadata": {"GENERATOR": "grpc"}},
                "certificate_providers": {
                    "google_cloud_private_spiffe": {
                        "plugin_name": "file_watcher",
                        "config": {"certificate_file": "/etc/certs/cert.pem"}
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(equivalent, built);
    }

    #[test]
    fn builder_runs_the_same_validation_as_json() {
        let err = BootstrapConfig::builder("").build().unwrap_err();
        assert!(matches!(err, BootstrapError::Validation(_)));
        assert!(err.to_string().contains("server_uri must not be empty"));
    }

    #[test]
    fn builder_reports_unrepresentable_metadata_at_build() {
        // JSON object keys must be strings, so a tuple-keyed map has no JSON
        // form and `build` reports it.
        let unrepresentable = HashMap::from([((1u8, 2u8), "v")]);
        let err = BootstrapConfig::builder("xds:443")
            .node_metadata("bad", unrepresentable)
            .build()
            .unwrap_err();

        assert!(matches!(
            err,
            BootstrapError::Serialization {
                field: "node.metadata",
                ..
            }
        ));
    }

    #[test]
    fn builder_reports_only_the_first_serialization_error() {
        let unrepresentable = HashMap::from([((1u8, 2u8), "v")]);
        let err = BootstrapConfig::builder("xds:443")
            .node_metadata("bad", unrepresentable.clone())
            .certificate_provider("bad", "file_watcher", unrepresentable)
            .build()
            .unwrap_err();

        assert!(matches!(
            err,
            BootstrapError::Serialization {
                field: "node.metadata",
                ..
            }
        ));
    }

    #[test]
    fn parsed_unsupported_creds_are_skipped_but_rejected_by_the_builder() {
        // Parsing keeps forward compatibility: an unknown type is skipped and
        // the next supported one wins.
        let parsed = BootstrapConfig::from_json(
            r#"{
                "xds_servers": [{
                    "server_uri": "xds:443",
                    "channel_creds": [{"type": "future_creds"}, {"type": "tls"}]
                }]
            }"#,
        )
        .unwrap();
        assert!(parsed.use_tls());

        // Building states what this client should use, so `build` rejects the
        // same value parsing skips. `Deserialize` is the only way to name one.
        let err = BootstrapConfig::builder("xds:443")
            .channel_creds(
                parsed.xds_servers[0]
                    .channel_creds
                    .iter()
                    .map(|c| c.cred_type.clone()),
            )
            .build()
            .unwrap_err();

        assert!(matches!(err, BootstrapError::Validation(_)));
        assert!(err.to_string().contains("future_creds"));
    }

    #[test]
    fn deserializing_a_config_directly_runs_validation() {
        // `BootstrapConfig` derives `Deserialize` so callers can embed it in
        // their own config structs; that path validates too, keeping
        // `server_uri` total.
        let err =
            serde_json::from_str::<BootstrapConfig>(r#"{"xds_servers": []}"#).expect_err("empty");
        assert!(err.to_string().contains("xds_servers must not be empty"));

        let ok = serde_json::from_str::<BootstrapConfig>(
            r#"{"xds_servers": [{"server_uri": "xds:443"}]}"#,
        )
        .unwrap();
        assert_eq!(ok.server_uri(), "xds:443");
    }

    #[test]
    fn builder_setters_replace_rather_than_accumulate() {
        let built = BootstrapConfig::builder("xds:443")
            .channel_creds([ChannelCredentialType::Insecure])
            .channel_creds([ChannelCredentialType::Tls])
            .node_metadata("k", "first")
            .node_metadata("k", "second")
            .build()
            .unwrap();

        assert!(built.use_tls());
        assert_eq!(built.node.metadata["k"], serde_json::json!("second"));
    }

    #[test]
    fn parse_node_metadata() {
        let json = r#"{
            "xds_servers": [{"server_uri": "localhost:5000"}],
            "node": {
                "id": "n1",
                "metadata": {
                    "GENERATOR": "grpc",
                    "PILOT_VERSION": "1.20"
                }
            }
        }"#;
        let config = BootstrapConfig::from_json(json).unwrap();
        assert_eq!(
            config.node.metadata.get("GENERATOR").unwrap(),
            &serde_json::Value::String("grpc".to_string())
        );
        assert_eq!(
            config.node.metadata.get("PILOT_VERSION").unwrap(),
            &serde_json::Value::String("1.20".to_string())
        );
    }

    #[test]
    fn node_from_config_propagates_metadata() {
        let json = r#"{
            "xds_servers": [{"server_uri": "localhost:5000"}],
            "node": {
                "id": "n1",
                "metadata": {"GENERATOR": "grpc"}
            }
        }"#;
        let config = BootstrapConfig::from_json(json).unwrap();
        let node = Node::try_from(config.node).unwrap();
        assert_eq!(
            node.metadata.get("GENERATOR").unwrap(),
            &MetadataValue::String("grpc".to_string())
        );
    }

    #[test]
    fn parse_istio_style_metadata() {
        // Real-world Istio bootstrap shape: nested objects, numbers, arrays.
        let json = r#"{
            "xds_servers": [{"server_uri": "unix:///etc/istio/proxy/XDS"}],
            "node": {
                "id": "sidecar~10.0.0.1~pod.ns~ns.svc.cluster.local",
                "metadata": {
                    "GENERATOR": "grpc",
                    "ANNOTATIONS": {
                        "inject.istio.io/templates": "grpc-agent",
                        "istio.io/rev": "default"
                    },
                    "CLUSTER_ID": "Kubernetes",
                    "ENVOY_PROMETHEUS_PORT": 15090,
                    "PILOT_SAN": ["istiod.istio-system.svc"]
                }
            }
        }"#;
        let config = BootstrapConfig::from_json(json).unwrap();
        let node = Node::try_from(config.node).unwrap();

        assert_eq!(
            node.metadata.get("GENERATOR").unwrap(),
            &MetadataValue::String("grpc".to_string())
        );
        assert_eq!(
            node.metadata.get("ENVOY_PROMETHEUS_PORT").unwrap(),
            &MetadataValue::Number(15090.0)
        );

        match node.metadata.get("ANNOTATIONS").unwrap() {
            MetadataValue::Object(fields) => {
                assert_eq!(
                    fields.get("istio.io/rev").unwrap(),
                    &MetadataValue::String("default".to_string())
                );
            }
            other => panic!("expected Object, got {other:?}"),
        }

        match node.metadata.get("PILOT_SAN").unwrap() {
            MetadataValue::Array(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(
                    &items[0],
                    &MetadataValue::String("istiod.istio-system.svc".to_string())
                );
            }
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[test]
    fn missing_metadata_defaults_to_empty() {
        let config = BootstrapConfig::from_json(minimal_json()).unwrap();
        assert!(config.node.metadata.is_empty());
        let node = Node::try_from(config.node).unwrap();
        assert!(node.metadata.is_empty());
    }

    #[test]
    fn parse_certificate_providers() {
        let json = r#"{
            "xds_servers": [{"server_uri": "localhost:5000"}],
            "certificate_providers": {
                "google_cloud_private_spiffe": {
                    "plugin_name": "file_watcher",
                    "config": {
                        "certificate_file": "/var/run/certs/certificates.pem",
                        "private_key_file": "/var/run/certs/private_key.pem",
                        "ca_certificate_file": "/var/run/certs/ca_certificates.pem",
                        "refresh_interval": "60s"
                    }
                }
            }
        }"#;
        let config = BootstrapConfig::from_json(json).unwrap();
        assert_eq!(config.certificate_providers.len(), 1);

        let plugin = &config.certificate_providers["google_cloud_private_spiffe"];
        assert_eq!(plugin.plugin_name, "file_watcher");
        assert_eq!(
            plugin.config["certificate_file"],
            "/var/run/certs/certificates.pem"
        );
        assert_eq!(
            plugin.config["ca_certificate_file"],
            "/var/run/certs/ca_certificates.pem"
        );
        assert_eq!(plugin.config["refresh_interval"], "60s");
    }

    #[test]
    fn missing_certificate_providers_defaults_to_empty() {
        let config = BootstrapConfig::from_json(minimal_json()).unwrap();
        assert!(config.certificate_providers.is_empty());
    }

    #[test]
    fn parse_google_default() {
        let json = r#"{
            "xds_servers": [{
                "server_uri": "xds.example.com:443",
                "channel_creds": [{"type": "google_default"}]
            }],
            "node": {"id": "n1"}
        }"#;
        let config = BootstrapConfig::from_json(json).unwrap();
        assert_eq!(
            config.xds_servers[0].channel_creds[0].cred_type,
            ChannelCredentialType::GoogleDefault
        );
        assert!(config.use_tls());
        assert_eq!(
            config.selected_credential(),
            Some(&ChannelCredentialType::GoogleDefault)
        );
    }

    #[test]
    fn multiple_certificate_provider_instances() {
        let json = r#"{
            "xds_servers": [{"server_uri": "localhost:5000"}],
            "certificate_providers": {
                "identity": {
                    "plugin_name": "file_watcher",
                    "config": {
                        "certificate_file": "/certs/cert.pem",
                        "private_key_file": "/certs/key.pem"
                    }
                },
                "root_ca": {
                    "plugin_name": "file_watcher",
                    "config": {
                        "ca_certificate_file": "/certs/ca.pem"
                    }
                }
            }
        }"#;
        let config = BootstrapConfig::from_json(json).unwrap();
        assert_eq!(config.certificate_providers.len(), 2);
        assert!(config.certificate_providers.contains_key("identity"));
        assert!(config.certificate_providers.contains_key("root_ca"));
    }
}
