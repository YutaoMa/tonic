//! Tokio runtime for the XDS client.
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::runtime::Runtime;

/// Tokio runtime.
#[derive(Clone, Debug)]
pub struct TokioRuntime;

impl Runtime for TokioRuntime {
    fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(future);
    }

    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(tokio::time::sleep(duration))
    }
}
