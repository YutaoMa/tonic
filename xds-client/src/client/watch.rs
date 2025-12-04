//! Watcher for XDS resources.
use std::pin::Pin;
use std::task::{Context, Poll};
use futures::{Stream, StreamExt};
use std::sync::{Arc, Mutex};

use crate::error::Error;

/// The update type for the XDS watcher.
#[derive(Debug)]
pub enum XdsUpdate<T> {
    /// Set the value of a resource.
    Set(T),
    /// Remove the value of a resource.
    Remove,
    /// An error occurred while watching a resource.
    Error(Error),
}

/// The XDS watcher.
#[derive(Debug)]
pub struct XdsWatcher<T: Clone + Send + Sync + 'static> {
    rx: futures::channel::mpsc::UnboundedReceiver<XdsUpdate<T>>,
    value: Arc<Mutex<Option<T>>>,
}

impl<T: Clone + Send + Sync + 'static> XdsWatcher<T> {
    /// Create a new XDS watcher.
    pub fn new(
        rx: futures::channel::mpsc::UnboundedReceiver<XdsUpdate<T>>,
        value: Arc<Mutex<Option<T>>>
    ) -> Self {
        Self {
            rx,
            value,
        }
    }

    /// Get the current value of the XDS watcher.
    pub fn get(&self) -> Option<T> {
        let lock = self.value.lock().unwrap();
        lock.clone()
    }
}

impl<T: Clone + Send + Sync + 'static> Stream for XdsWatcher<T> {
    type Item = XdsUpdate<T>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().rx.poll_next_unpin(cx)
    }
}
