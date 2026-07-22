use crate::common::async_util::BoxFuture;
use crate::xds::resource::route_config::{RouteConfigMetadata, RouteConfigResource};
use crate::xds::routing::{RouteConfigWatcher, RoutingError};
use http::Request;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{BoxError, Layer, Service};

/// Represents the input for routing decisions.
#[allow(dead_code)]
pub(crate) struct RouteInput<'a> {
    /// The authority (host) of the request URI.
    pub authority: &'a str,
    /// The HTTP headers of the request. These can be used for header-based routing decisions.
    pub headers: &'a http::HeaderMap,
    /// Route configuration already bound to this request by an upstream
    /// [`RouteConfigSelectorService`]. When `Some`, the router matches against
    /// it directly (the single-pass path shared with a pre-route interceptor);
    /// when `None`, routing fails with `RoutingError::NotReady` (the selector
    /// layer always sets it in the assembled channel stack).
    pub config: Option<&'a RouteConfigResource>,
}

/// Represents the routing decision made by the routing layer.
#[derive(Clone, Debug)]
pub(crate) struct RouteDecision {
    /// The name of the cluster to which the request should be routed.
    pub cluster: String,
    /// The request hash computed from the route's hash policies (gRFC A42),
    /// consumed by the ring-hash LB picker. `None` when no hash policy produced
    /// a hash, in which case the picker falls back to a random hash.
    // Populated by the routing layer; consumed by the ring-hash picker (later PR).
    #[allow(dead_code)]
    pub request_hash: Option<u64>,
}

/// Marker stored in request extensions carrying the [`RouteConfigResource`]
/// bound to a request by [`RouteConfigSelectorService`].
///
/// Binding the config once and sharing it via extensions guarantees that a
/// [`PreRouteInterceptor`] and the router observe the *same* route-config
/// version for a given request, even if an xDS update lands mid-request.
#[derive(Clone)]
pub(crate) struct ActiveRouteConfig(pub(crate) Arc<RouteConfigResource>);

/// A hook that runs **before** xDS route selection.
///
/// The interceptor may inspect and mutate the request headers using the
/// [`RouteConfigMetadata`] attached to the active `RouteConfiguration`. Because
/// it runs before routing, any header mutation it makes is visible to route
/// matching — enabling config-driven request transformation, such as computing
/// a partition/shard key and injecting a routing header that the standard
/// header-match router then selects on.
///
/// The hook deliberately cannot return a routing decision: it influences routing
/// only by mutating the request, after which the standard xDS router runs once.
/// This keeps the routing model itself unchanged and single-pass.
pub trait PreRouteInterceptor: Send + Sync + 'static {
    /// Inspects and optionally mutates `headers` using the active route-config
    /// `metadata`. Runs before route selection.
    fn on_request(&self, headers: &mut http::HeaderMap, metadata: &RouteConfigMetadata);
}

/// Trait for routing requests to clusters.
///
/// Implementations resolve a request's authority and headers into a target
/// cluster name. The xDS-backed implementation is
/// [`XdsRouter`](crate::xds::routing::XdsRouter).
pub(crate) trait Router: Send + Sync + 'static {
    fn route(&self, input: &RouteInput<'_>) -> BoxFuture<Result<RouteDecision, RoutingError>>;
}

/// Tower service for routing requests to the appropriate cluster.
/// Attaches routing decision as [`RouteDecision`] to the request extensions.
/// The [`RouteDecision`] will be used by the `XdsLbService` to identify the
/// cluster to which the request should be routed.
#[derive(Clone)]
pub(crate) struct XdsRoutingService<S> {
    /// The inner Tower service to which the request will be forwarded after routing decision is made.
    inner: S,
    /// The router used to make routing decisions based on the request.
    router: Arc<dyn Router>,
    /// Channel-level authority used as the routing key.
    authority: Arc<str>,
}

impl<S, B> Service<Request<B>> for XdsRoutingService<S>
where
    S: Service<Request<B>, Error: Into<BoxError>> + Clone + Send + 'static,
    B: Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = BoxFuture<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, mut request: Request<B>) -> Self::Future {
        let router = self.router.clone();
        let authority = self.authority.clone();
        let mut inner_service = self.inner.clone();
        Box::pin(async move {
            let active = request.extensions().get::<ActiveRouteConfig>().cloned();
            let route_future = {
                let route_input = RouteInput {
                    authority: &authority,
                    headers: request.headers(),
                    config: active.as_ref().map(|a| a.0.as_ref()),
                };
                router.route(&route_input)
            };
            let route_decision = route_future.await?;
            request.extensions_mut().insert(route_decision);
            inner_service.call(request).await.map_err(Into::into)
        })
    }
}

/// Tower layer for routing requests to the appropriate cluster.
#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct XdsRoutingLayer {
    router: Arc<dyn Router>,
    authority: Arc<str>,
}

impl XdsRoutingLayer {
    /// Creates a new `XdsRoutingLayer` with the given [`Router`] and authority.
    ///
    /// `authority` is the routing key matched against `VirtualHost.domains`
    /// in RDS. It should be the endpoint portion of the xDS target.
    #[allow(dead_code)]
    pub(crate) fn new(router: Arc<dyn Router>, authority: Arc<str>) -> Self {
        Self { router, authority }
    }
}

impl<S> Layer<S> for XdsRoutingLayer {
    type Service = XdsRoutingService<S>;

    fn layer(&self, service: S) -> Self::Service {
        XdsRoutingService {
            inner: service,
            router: self.router.clone(),
            authority: self.authority.clone(),
        }
    }
}

/// Tower service that snapshots the active route configuration once and binds it
/// to the request as [`ActiveRouteConfig`], before any pre-route interceptor or
/// the router runs.
///
/// It also owns A57 initial-resource readiness: the first request blocks here
/// until route config is available, after which the stateless router and
/// any interceptor share this single snapshot.
#[derive(Clone)]
pub(crate) struct RouteConfigSelectorService<S> {
    inner: S,
    watcher: Arc<RouteConfigWatcher>,
}

impl<S, B> Service<Request<B>> for RouteConfigSelectorService<S>
where
    S: Service<Request<B>, Error: Into<BoxError>> + Clone + Send + 'static,
    B: Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = BoxFuture<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, mut request: Request<B>) -> Self::Future {
        let watcher = self.watcher.clone();
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let config = watcher.snapshot().await?;
            request.extensions_mut().insert(ActiveRouteConfig(config));
            inner.call(request).await.map_err(Into::into)
        })
    }
}

/// Layer producing a [`RouteConfigSelectorService`].
#[derive(Clone)]
pub(crate) struct RouteConfigSelectorLayer {
    watcher: Arc<RouteConfigWatcher>,
}

impl RouteConfigSelectorLayer {
    pub(crate) fn new(watcher: Arc<RouteConfigWatcher>) -> Self {
        Self { watcher }
    }
}

impl<S> Layer<S> for RouteConfigSelectorLayer {
    type Service = RouteConfigSelectorService<S>;

    fn layer(&self, service: S) -> Self::Service {
        RouteConfigSelectorService {
            inner: service,
            watcher: self.watcher.clone(),
        }
    }
}

/// Tower service that runs a [`PreRouteInterceptor`] before routing.
///
/// Reads the [`ActiveRouteConfig`] bound upstream and lets the interceptor
/// mutate request headers using its metadata; the mutated headers are then seen
/// by the router in the same pass.
#[derive(Clone)]
pub(crate) struct PreRouteService<S> {
    inner: S,
    interceptor: Arc<dyn PreRouteInterceptor>,
}

impl<S, B> Service<Request<B>> for PreRouteService<S>
where
    S: Service<Request<B>, Error: Into<BoxError>> + Clone + Send + 'static,
    B: Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = BoxFuture<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, mut request: Request<B>) -> Self::Future {
        if let Some(active) = request.extensions().get::<ActiveRouteConfig>().cloned() {
            self.interceptor
                .on_request(request.headers_mut(), &active.0.metadata);
        }
        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(request).await.map_err(Into::into) })
    }
}

/// Layer producing a [`PreRouteService`].
#[derive(Clone)]
pub(crate) struct PreRouteLayer {
    interceptor: Arc<dyn PreRouteInterceptor>,
}

impl PreRouteLayer {
    pub(crate) fn new(interceptor: Arc<dyn PreRouteInterceptor>) -> Self {
        Self { interceptor }
    }
}

impl<S> Layer<S> for PreRouteLayer {
    type Service = PreRouteService<S>;

    fn layer(&self, service: S) -> Self::Service {
        PreRouteService {
            inner: service,
            interceptor: self.interceptor.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tower::ServiceExt;
    use tower::service_fn;

    /// Mock router that records the `authority` it was called with.
    struct CaptureAuthorityRouter {
        captured: Arc<Mutex<Option<String>>>,
    }

    impl Router for CaptureAuthorityRouter {
        fn route(&self, input: &RouteInput<'_>) -> BoxFuture<Result<RouteDecision, RoutingError>> {
            *self.captured.lock().unwrap() = Some(input.authority.to_string());
            Box::pin(async move {
                Ok(RouteDecision {
                    cluster: "test-cluster".to_string(),
                    request_hash: None,
                })
            })
        }
    }

    /// Verifies the routing layer always sources `authority` from its layer
    /// config, not from the request URI.
    #[tokio::test]
    async fn uses_layer_authority_regardless_of_request_uri() {
        let captured = Arc::new(Mutex::new(None));
        let router: Arc<dyn Router> = Arc::new(CaptureAuthorityRouter {
            captured: captured.clone(),
        });
        let layer = XdsRoutingLayer::new(router, Arc::from("greeter.svc:50051"));

        let inner =
            service_fn(
                |_req: Request<()>| async move { Ok::<_, BoxError>(http::Response::new(())) },
            );
        let svc = layer.layer(inner);

        // Case 1: request with no authority on the URI (typical tonic-generated
        // client — see `tonic/src/client/grpc.rs::prepare_request`).
        let req = Request::builder()
            .uri("/pkg.Greeter/SayHello")
            .body(())
            .unwrap();
        svc.clone().oneshot(req).await.unwrap();
        assert_eq!(
            captured.lock().unwrap().as_deref(),
            Some("greeter.svc:50051"),
        );

        // Case 2: request with a different authority on the URI — the layer
        // must still use its own configured authority.
        *captured.lock().unwrap() = None;
        let req = Request::builder()
            .uri("http://other.example:443/pkg.Greeter/SayHello")
            .body(())
            .unwrap();
        svc.oneshot(req).await.unwrap();
        assert_eq!(
            captured.lock().unwrap().as_deref(),
            Some("greeter.svc:50051"),
        );
    }

    #[tokio::test]
    async fn pre_route_interceptor_drives_partition_selection() {
        use crate::xds::cache::XdsCache;
        use crate::xds::resource::route_config::{
            HeaderMatchSpecifierConfig, HeaderMatcherConfig, PathSpecifierConfig, RouteConfig,
            RouteConfigAction, RouteConfigMatch, RouteConfigResource, VirtualHostConfig,
        };
        use crate::xds::routing::XdsRouter;
        use envoy_types::pb::envoy::config::core::v3::Metadata;
        use envoy_types::pb::google::protobuf::Struct;

        // Route table: x-partition N -> cluster-pN, matched via integer range.
        fn partition_route(partition: i64, cluster: &str) -> RouteConfig {
            RouteConfig {
                match_criteria: RouteConfigMatch {
                    path_specifier: PathSpecifierConfig::Prefix("/".into()),
                    headers: vec![HeaderMatcherConfig {
                        name: "x-partition".into(),
                        match_specifier: HeaderMatchSpecifierConfig::Range {
                            start: partition,
                            end: partition + 1,
                        },
                        invert_match: false,
                    }],
                    case_sensitive: true,
                    match_fraction: None,
                },
                action: RouteConfigAction::Cluster(cluster.into()),
            }
        }

        // Config carries filter_metadata that the interceptor is expected to see.
        let mut filter_metadata = std::collections::HashMap::new();
        filter_metadata.insert("partitioning".to_string(), Struct::default());
        let metadata = RouteConfigMetadata::from_proto(Metadata {
            filter_metadata,
            ..Default::default()
        });

        let rc = Arc::new(RouteConfigResource {
            name: "rc".into(),
            virtual_hosts: vec![VirtualHostConfig {
                name: "vh".into(),
                domains: vec!["*".into()],
                routes: vec![
                    partition_route(1, "cluster-p1"),
                    partition_route(2, "cluster-p2"),
                ],
            }],
            metadata,
        });

        let cache = XdsCache::new();
        cache.update_route_config(rc);
        tokio::task::yield_now().await;

        /// Reads the `hint` header, verifies metadata is delivered, and injects
        /// the partition header the router selects on.
        struct PartitionInterceptor;
        impl PreRouteInterceptor for PartitionInterceptor {
            fn on_request(&self, headers: &mut http::HeaderMap, metadata: &RouteConfigMetadata) {
                assert!(
                    metadata.filter_metadata("partitioning").is_some(),
                    "interceptor must see the RouteConfiguration metadata",
                );
                let partition = match headers.get("hint").and_then(|v| v.to_str().ok()) {
                    Some("a") => "1",
                    _ => "2",
                };
                headers.insert("x-partition", partition.parse().unwrap());
            }
        }

        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let terminal = {
            let captured = captured.clone();
            service_fn(move |req: Request<()>| {
                let captured = captured.clone();
                async move {
                    let cluster = req
                        .extensions()
                        .get::<RouteDecision>()
                        .map(|d| d.cluster.clone());
                    *captured.lock().unwrap() = cluster;
                    Ok::<_, BoxError>(http::Response::new(()))
                }
            })
        };

        let watcher = Arc::new(RouteConfigWatcher::new(&cache));
        let router: Arc<dyn Router> = Arc::new(XdsRouter);
        let interceptor: Arc<dyn PreRouteInterceptor> = Arc::new(PartitionInterceptor);
        let svc = RouteConfigSelectorLayer::new(watcher).layer(
            PreRouteLayer::new(interceptor)
                .layer(XdsRoutingLayer::new(router, Arc::from("svc")).layer(terminal)),
        );

        // hint "a" -> partition 1 -> cluster-p1
        let req = Request::builder().header("hint", "a").body(()).unwrap();
        svc.clone().oneshot(req).await.unwrap();
        assert_eq!(captured.lock().unwrap().as_deref(), Some("cluster-p1"));

        // hint "b" -> partition 2 -> cluster-p2
        let req = Request::builder().header("hint", "b").body(()).unwrap();
        svc.oneshot(req).await.unwrap();
        assert_eq!(captured.lock().unwrap().as_deref(), Some("cluster-p2"));
    }
}
