//! Worker for the XDS client.
use bytes::Bytes;
use crate::client::config::ClientConfig;
use crate::runtime::Runtime;
use crate::transport::{XdsDiscoveryRequest, XdsDiscoveryResponse, TransportFactory};
use crate::error::{Error, Result};
use futures::channel::mpsc;
use futures::{FutureExt, SinkExt, StreamExt};
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, info};

/// Handler for resource updates.
pub trait ResourceUpdateHandler: Send + Sync + std::fmt::Debug {
    /// Handle an update to a resource.
    fn on_update(&self, payload: Bytes) -> Result<()>;
    /// Handle an error while watching a resource.
    fn on_error(&self, error: Error);
}

/// Command for the XDS worker.
#[derive(Debug)]
pub enum Command {
    /// Watch a resource.
    Watch {
        type_url: String,
        resource_names: Vec<String>,
        handler: Box<dyn ResourceUpdateHandler>,
    },
}

/// Worker for the XDS client.
#[derive(Debug)]
pub(crate) struct SotwWorker<R, T> {
    config: ClientConfig,
    runtime: R,
    transport_factory: T,
    cmd_rx: mpsc::UnboundedReceiver<Command>,
    subscriptions: HashMap<String, Subscription>,
    versions: HashMap<String, (String, String)>,
}

#[derive(Debug)]
struct Subscription {
    resources: HashSet<String>,
    handlers: Vec<Box<dyn ResourceUpdateHandler>>,
}

impl<R: Runtime, T: TransportFactory> SotwWorker<R, T> {
    /// Create a new worker.
    pub(crate) fn new(
        config: ClientConfig,
        runtime: R,
        transport_factory: T,
        cmd_rx: mpsc::UnboundedReceiver<Command>,
    ) -> Self {
        Self {
            config,
            runtime,
            transport_factory,
            cmd_rx,
            subscriptions: HashMap::new(),
            versions: HashMap::new(),
        }
    }

    pub(crate) async fn run(mut self) {
        loop {
            info!("Connecting to xDS server at {}", self.config.server_uri);
            match self.transport_factory.create_stream().await {
                Ok(stream) => {
                    info!("Connected.");
                    if let Err(e) = self.work(stream).await {
                        error!("Stream error: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to connect: {}", e);
                }
            }

            self.runtime.sleep(self.config.connect_timeout).await;
        }
    }

    async fn work(&mut self, mut stream: T::Stream) -> Result<()> {
        // Send initial requests
        for (type_url, sub) in &self.subscriptions {
            send_request(
                &mut stream,
                &self.config,
                type_url,
                &sub.resources,
                "",
                "",
            )
            .await?;
        }

        loop {
            futures::select! {
                cmd = self.cmd_rx.next().fuse() => {
                    match cmd {
                        Some(Command::Watch { type_url, resource_names, handler }) => {
                            debug!("Received watch for type: {}", type_url);
                            let sub = self.subscriptions.entry(type_url.clone()).or_insert(Subscription {
                                resources: HashSet::new(),
                                handlers: Vec::new(),
                            });

                            for name in &resource_names {
                                sub.resources.insert(name.clone());
                            }
                            sub.handlers.push(handler);

                            // Get version/nonce info - separate logic to avoid borrow conflict
                            let (version, nonce) = self.versions.get(&type_url).cloned().unwrap_or_default();
                            send_request(&mut stream, &self.config, &type_url, &sub.resources, &version, &nonce).await?;
                        }
                        None => return Ok(()),
                    }
                }
                resp = stream.next().fuse() => {
                    match resp {
                        Some(Ok(response)) => {
                            self.handle_response(&mut stream, response).await?;
                        }
                        Some(Err(e)) => return Err(e),
                        None => return Err(Error::Transport("Stream closed".into())),
                    }
                }
            }
        }
    }

    async fn handle_response(
        &mut self,
        stream: &mut T::Stream,
        resp: XdsDiscoveryResponse,
    ) -> Result<()> {
        debug!("Received update for type: {}", resp.type_url);
        let type_url = resp.type_url.clone();
        let version = resp.version_info.clone();
        let nonce = resp.nonce.clone();

        let mut resources_to_ack = HashSet::new();

        if let Some(sub) = self.subscriptions.get(&type_url) {
            resources_to_ack = sub.resources.clone();

            for resource_bytes in &resp.resources {
                for handler in &sub.handlers {
                    if let Err(e) = handler.on_update(resource_bytes.clone()) {
                        error!("Handler error: {}", e);
                    }
                }
            }
        }

        if !resources_to_ack.is_empty() {
            self.versions
                .insert(type_url.clone(), (version.clone(), nonce.clone()));
            send_request(
                stream,
                &self.config,
                &type_url,
                &resources_to_ack,
                &version,
                &nonce,
            )
            .await?;
        }

        Ok(())
    }
}

async fn send_request<S>(
    stream: &mut S,
    config: &ClientConfig,
    type_url: &str,
    resources: &HashSet<String>,
    version: &str,
    nonce: &str,
) -> Result<()>
where
    S: futures::Sink<XdsDiscoveryRequest, Error = Error> + Unpin,
{
    let req = XdsDiscoveryRequest {
        version_info: version.to_string(),
        node_id: config.node_id.clone(),
        resource_names: resources.iter().cloned().collect(),
        type_url: type_url.to_string(),
        response_nonce: nonce.to_string(),
        error_detail: None,
    };
    stream.send(req).await
}
