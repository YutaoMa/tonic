//! Provides a client for watching XDS resources and receiving updates.
use crate::{client::watch::XdsWatcher, resource::XdsResource};
use crate::client::worker::{Command, ResourceUpdateHandler};
use crate::client::watch::XdsUpdate;
use crate::error::{Error, Result};
use futures::channel::mpsc;
use std::sync::{Arc, Mutex};
use bytes::Bytes;

pub mod builder;
pub mod config;
pub mod watch;
pub(crate) mod worker;

pub use config::{ClientConfig, TlsConfig};

/// The XDS client.
#[derive(Clone, Debug)]
pub struct XdsClient {
    cmd_tx: mpsc::UnboundedSender<Command>,
}

impl XdsClient {
    /// Create a new XDS client.
    pub fn new(cmd_tx: mpsc::UnboundedSender<Command>) -> Self {
        Self { cmd_tx }
    }

    /// Watch a resource with the given name.
    pub async fn watch<T: XdsResource>(&self, name: String) -> Result<XdsWatcher<T::Resource>> {
        let (tx, rx) = mpsc::unbounded();
        let value = Arc::new(Mutex::new(None));
        
        let handler = HandlerAdapter::<T> {
            tx,
            value: value.clone(),
            _marker: std::marker::PhantomData,
        };

        let cmd = Command::Watch {
            type_url: T::type_url().to_string(),
            resource_names: vec![name],
            handler: Box::new(handler),
        };

        self.cmd_tx.unbounded_send(cmd)
            .map_err(|_| Error::Watch("Client worker is closed".into()))?;
        
        Ok(XdsWatcher::new(rx, value))
    }
}

#[derive(Debug)]
struct HandlerAdapter<T: XdsResource> {
    tx: mpsc::UnboundedSender<XdsUpdate<T::Resource>>,
    value: Arc<Mutex<Option<T::Resource>>>,
    _marker: std::marker::PhantomData<T>,
}

impl<T: XdsResource> ResourceUpdateHandler for HandlerAdapter<T> {
    fn on_update(&self, payload: Bytes) -> Result<()> {
        let resource = T::decode(&payload)?;
        
        // Update shared value safe handling of poisoned lock
        {
            let mut lock = self.value.lock().map_err(|_| Error::Watch("Lock poisoned".into()))?;
            *lock = Some(resource.clone());
        }

        // Notify stream
        let _ = self.tx.unbounded_send(XdsUpdate::Set(resource));
        Ok(())
    }

    fn on_error(&self, error: Error) {
        let _ = self.tx.unbounded_send(XdsUpdate::Error(error));
    }
}
