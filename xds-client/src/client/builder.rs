//! Builder for the XDS client.
use crate::client::config::ClientConfig;
use crate::client::worker::SotwWorker;
use crate::client::XdsClient;
use crate::error::Error;
use crate::runtime::Runtime;
use crate::transport::TransportFactory;

/// Builder for the XDS client.
#[derive(Debug)]
pub struct XdsClientBuilder {
    config: ClientConfig,
}

impl XdsClientBuilder {
    /// Create a new builder with the given configuration.
    pub fn new(config: ClientConfig) -> Self {
        Self { config }
    }

    /// Build the XDS client with the given runtime and transport.
    pub async fn build_with<R, T>(self, runtime: R, transport: T) -> Result<XdsClient, Error>
    where
        R: Runtime + Clone,
        T: TransportFactory + 'static,
    {
        let (cmd_tx, cmd_rx) = futures::channel::mpsc::unbounded();
        let worker = SotwWorker::new(self.config, runtime.clone(), transport, cmd_rx);

        runtime.spawn(async move {
            worker.run().await;
        });

        Ok(XdsClient::new(cmd_tx))
    }
}
