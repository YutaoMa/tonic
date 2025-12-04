//! Tonic transport for the XDS client.
use crate::client::config::ClientConfig;
use crate::pb::envoy::service::discovery::v3::aggregated_discovery_service_client::AggregatedDiscoveryServiceClient;
use crate::pb::envoy::service::discovery::v3::DiscoveryResponse;
use crate::transport::{
    TransportFactory, XdsDiscoveryRequest, XdsDiscoveryResponse,
};
use crate::error::{Error, Result};
use futures::future::BoxFuture;
use futures::{Sink, Stream};
use std::pin::Pin;
use std::task::{Context, Poll};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

/// Tonic transport factory.
#[derive(Debug)]
pub struct TonicTransportFactory {
    channel: Channel,
}

impl TonicTransportFactory {
    /// Create a new Tonic transport factory.
    pub async fn new(config: &ClientConfig) -> Result<Self> {
        let mut endpoint = Channel::from_shared(config.server_uri.clone())
            .map_err(|e| Error::Config(format!("Invalid URI: {}", e)))?;

        if let Some(tls) = &config.tls_config {
            let mut tls_config = ClientTlsConfig::new();

            if let Some(ca) = &tls.ca_cert_pem {
                tls_config = tls_config.ca_certificate(Certificate::from_pem(ca));
            }

            if let (Some(cert), Some(key)) = (&tls.client_cert_pem, &tls.client_key_pem) {
                tls_config = tls_config.identity(Identity::from_pem(cert, key));
            }

            if let Some(domain) = &tls.domain_name {
                tls_config = tls_config.domain_name(domain);
            }

            endpoint = endpoint.tls_config(tls_config)?;
        }

        let channel = endpoint.connect().await?;
        Ok(Self { channel })
    }
}

#[async_trait::async_trait]
impl TransportFactory for TonicTransportFactory {
    type Stream = TonicSotwStream;

    async fn create_stream(&self) -> Result<Self::Stream> {
        let mut client = AggregatedDiscoveryServiceClient::new(self.channel.clone());

        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let req_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

        // Lazy stream establishment
        let fut = Box::pin(async move { client.stream_aggregated_resources(req_stream).await });

        Ok(TonicSotwStream {
            req_tx: tx,
            state: StreamState::Handshaking(fut),
        })
    }
}

/// State of the Tonic stream.
enum StreamState {
    Handshaking(
        BoxFuture<
            'static,
            std::result::Result<tonic::Response<tonic::Streaming<DiscoveryResponse>>, tonic::Status>,
        >,
    ),
    Streaming(tonic::Streaming<DiscoveryResponse>),
    Done,
}

impl std::fmt::Debug for StreamState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamState::Handshaking(_) => write!(f, "Handshaking"),
            StreamState::Streaming(_) => write!(f, "Streaming"),
            StreamState::Done => write!(f, "Done"),
        }
    }
}

/// Tonic stream for the XDS client.
#[derive(Debug)]
pub struct TonicSotwStream {
    req_tx: tokio::sync::mpsc::Sender<crate::pb::envoy::service::discovery::v3::DiscoveryRequest>,
    state: StreamState,
}

impl Stream for TonicSotwStream {
    type Item = Result<XdsDiscoveryResponse>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match &mut self.state {
                StreamState::Handshaking(fut) => match fut.as_mut().poll(cx) {
                    Poll::Ready(Ok(resp)) => {
                        self.state = StreamState::Streaming(resp.into_inner());
                    }
                    Poll::Ready(Err(e)) => {
                        self.state = StreamState::Done;
                        return Poll::Ready(Some(Err(Error::GrpcStatus(e))));
                    }
                    Poll::Pending => return Poll::Pending,
                },
                StreamState::Streaming(stream) => {
                    let res = Pin::new(stream).poll_next(cx);
                    match res {
                        Poll::Ready(Some(Ok(resp))) => {
                            let xds_resp = XdsDiscoveryResponse::try_from(resp)?;
                            return Poll::Ready(Some(Ok(xds_resp)));
                        }
                        Poll::Ready(Some(Err(e))) => {
                            return Poll::Ready(Some(Err(Error::GrpcStatus(e))));
                        }
                        Poll::Ready(None) => return Poll::Ready(None),
                        Poll::Pending => return Poll::Pending,
                    }
                }
                StreamState::Done => return Poll::Ready(None),
            }
        }
    }
}

impl Sink<XdsDiscoveryRequest> for TonicSotwStream {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: XdsDiscoveryRequest) -> std::result::Result<(), Self::Error> {
        let req = item.into();
        self.req_tx.try_send(req).map_err(|e| Error::Transport(Box::new(e)))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}
