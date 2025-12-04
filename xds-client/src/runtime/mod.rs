//! Runtime for the XDS client.
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub mod tokio;

/// Trait for runtimes.
pub trait Runtime: Send + Sync + std::fmt::Debug + 'static {
    /// Spawn a future.
    fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static;

    /// Sleep for a duration.
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}
