//! Converts snapshot-based endpoint cache updates into incremental
//! [`EndpointDelta`] streams.
//!
//! The resource manager writes [`EndpointsResource`] snapshots into the
//! [`XdsCache`]; this module diffs consecutive snapshots and produces
//! `EndpointDelta::Insert` / `EndpointDelta::Remove` events. The factory
//! layer in [`ClusterClientRegistry`] turns those deltas into
//! `Change<EndpointAddress, Service>` events for Tower's P2C balancer by
//! applying the cluster's [`Connector`] at Insert time.
//!
//! [`XdsCache`]: crate::xds::cache::XdsCache
//! [`Connector`]: crate::client::endpoint::Connector
//! [`ClusterClientRegistry`]: crate::client::cluster::ClusterClientRegistry

use std::collections::HashSet;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tower::BoxError;

use crate::client::endpoint::EndpointAddress;
use crate::client::lb::{EndpointDelta, EndpointStream};
use crate::xds::cache::CacheWatch;
use crate::xds::resource::EndpointsResource;

/// Buffer capacity for the endpoint delta channel between the diff loop
/// and the consumer.
const ENDPOINT_CHANNEL_CAPACITY: usize = 64;

/// Subscribes to a cluster's endpoint snapshots and returns an
/// [`EndpointStream`] of incremental [`EndpointDelta`]s.
///
/// Diffs each snapshot against the previous set of healthy endpoints, emitting
/// `Insert` for new endpoints and `Remove` for gone ones. The spawned diff task
/// exits naturally when either the `CacheWatch` closes (cluster removed from
/// cache) or the consumer drops the returned stream.
pub(crate) fn discover_endpoint_deltas(watch: CacheWatch<EndpointsResource>) -> EndpointStream {
    let (tx, rx) = mpsc::channel(ENDPOINT_CHANNEL_CAPACITY);
    tokio::spawn(diff_loop(watch, tx));
    Box::pin(ReceiverStream::new(rx))
}

async fn diff_loop(
    mut watch: CacheWatch<EndpointsResource>,
    tx: mpsc::Sender<Result<EndpointDelta, BoxError>>,
) {
    let mut active: HashSet<EndpointAddress> = HashSet::new();

    while let Some(endpoints) = watch.next().await {
        let new_set: HashSet<EndpointAddress> = endpoints
            .healthy_endpoints()
            .map(|ep| ep.address.clone())
            .collect();

        for added in new_set.difference(&active) {
            if tx
                .send(Ok(EndpointDelta::Insert(added.clone())))
                .await
                .is_err()
            {
                return;
            }
        }

        for removed in active.difference(&new_set) {
            if tx
                .send(Ok(EndpointDelta::Remove(removed.clone())))
                .await
                .is_err()
            {
                return;
            }
        }

        active = new_set;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xds::cache::XdsCache;
    use crate::xds::resource::endpoints::{HealthStatus, LocalityEndpoints, ResolvedEndpoint};
    use std::sync::Arc;
    use tokio_stream::StreamExt;

    fn make_endpoints(cluster: &str, addrs: &[(&str, u16)]) -> Arc<EndpointsResource> {
        Arc::new(EndpointsResource {
            cluster_name: cluster.to_string(),
            localities: vec![LocalityEndpoints {
                locality: None,
                endpoints: addrs
                    .iter()
                    .map(|(host, port)| ResolvedEndpoint {
                        address: EndpointAddress::new(*host, *port),
                        health_status: HealthStatus::Healthy,
                        load_balancing_weight: 1,
                    })
                    .collect(),
                load_balancing_weight: 100,
                priority: 0,
            }],
        })
    }

    #[tokio::test]
    async fn initial_endpoints_emitted_as_inserts() {
        let cache = XdsCache::new();
        cache.update_endpoints(
            "c1",
            make_endpoints("c1", &[("10.0.0.1", 8080), ("10.0.0.2", 8080)]),
        );

        let mut stream = discover_endpoint_deltas(cache.watch_endpoints("c1"));

        let mut addrs: Vec<String> = Vec::new();
        for _ in 0..2 {
            match stream.next().await.unwrap().unwrap() {
                EndpointDelta::Insert(addr) => addrs.push(addr.to_string()),
                EndpointDelta::Remove(_) => panic!("expected Insert"),
            }
        }
        addrs.sort();
        assert_eq!(addrs, vec!["10.0.0.1:8080", "10.0.0.2:8080"]);
    }

    #[tokio::test]
    async fn added_endpoint_emits_insert() {
        let cache = XdsCache::new();
        cache.update_endpoints("c1", make_endpoints("c1", &[("10.0.0.1", 8080)]));

        let mut stream = discover_endpoint_deltas(cache.watch_endpoints("c1"));
        let _ = stream.next().await; // consume initial

        cache.update_endpoints(
            "c1",
            make_endpoints("c1", &[("10.0.0.1", 8080), ("10.0.0.2", 8080)]),
        );

        match stream.next().await.unwrap().unwrap() {
            EndpointDelta::Insert(addr) => assert_eq!(addr.to_string(), "10.0.0.2:8080"),
            EndpointDelta::Remove(_) => panic!("expected Insert for new endpoint"),
        }
    }

    #[tokio::test]
    async fn removed_endpoint_emits_remove() {
        let cache = XdsCache::new();
        cache.update_endpoints(
            "c1",
            make_endpoints("c1", &[("10.0.0.1", 8080), ("10.0.0.2", 8080)]),
        );

        let mut stream = discover_endpoint_deltas(cache.watch_endpoints("c1"));
        let _ = stream.next().await;
        let _ = stream.next().await;

        cache.update_endpoints("c1", make_endpoints("c1", &[("10.0.0.1", 8080)]));

        match stream.next().await.unwrap().unwrap() {
            EndpointDelta::Remove(addr) => assert_eq!(addr.to_string(), "10.0.0.2:8080"),
            EndpointDelta::Insert(..) => panic!("expected Remove"),
        }
    }

    #[tokio::test]
    async fn unhealthy_endpoint_removed() {
        let cache = XdsCache::new();
        cache.update_endpoints("c1", make_endpoints("c1", &[("10.0.0.1", 8080)]));

        let mut stream = discover_endpoint_deltas(cache.watch_endpoints("c1"));
        let _ = stream.next().await;

        let unhealthy = Arc::new(EndpointsResource {
            cluster_name: "c1".to_string(),
            localities: vec![LocalityEndpoints {
                locality: None,
                endpoints: vec![ResolvedEndpoint {
                    address: EndpointAddress::new("10.0.0.1", 8080),
                    health_status: HealthStatus::Unhealthy,
                    load_balancing_weight: 1,
                }],
                load_balancing_weight: 100,
                priority: 0,
            }],
        });
        cache.update_endpoints("c1", unhealthy);

        match stream.next().await.unwrap().unwrap() {
            EndpointDelta::Remove(addr) => assert_eq!(addr.to_string(), "10.0.0.1:8080"),
            EndpointDelta::Insert(..) => panic!("expected Remove for unhealthy endpoint"),
        }
    }

    #[tokio::test]
    async fn cache_removal_closes_stream() {
        let cache = XdsCache::new();
        cache.update_endpoints("c1", make_endpoints("c1", &[("10.0.0.1", 8080)]));

        let mut stream = discover_endpoint_deltas(cache.watch_endpoints("c1"));
        let _ = stream.next().await;

        cache.remove_endpoints("c1");

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn multiple_clusters_independent() {
        let cache = XdsCache::new();
        cache.update_endpoints("c1", make_endpoints("c1", &[("10.0.0.1", 8080)]));
        cache.update_endpoints("c2", make_endpoints("c2", &[("10.0.0.2", 9090)]));

        let mut s1 = discover_endpoint_deltas(cache.watch_endpoints("c1"));
        let mut s2 = discover_endpoint_deltas(cache.watch_endpoints("c2"));

        match s1.next().await.unwrap().unwrap() {
            EndpointDelta::Insert(addr) => assert_eq!(addr.to_string(), "10.0.0.1:8080"),
            _ => panic!("expected Insert"),
        }
        match s2.next().await.unwrap().unwrap() {
            EndpointDelta::Insert(addr) => assert_eq!(addr.to_string(), "10.0.0.2:9090"),
            _ => panic!("expected Insert"),
        }
    }

    #[tokio::test]
    async fn endpoint_swap_emits_insert_then_remove() {
        let cache = XdsCache::new();
        cache.update_endpoints("c1", make_endpoints("c1", &[("10.0.0.1", 8080)]));

        let mut stream = discover_endpoint_deltas(cache.watch_endpoints("c1"));
        let _ = stream.next().await;

        cache.update_endpoints("c1", make_endpoints("c1", &[("10.0.0.2", 8080)]));

        let mut saw_remove = false;
        let mut saw_insert = false;
        for _ in 0..2 {
            match stream.next().await.unwrap().unwrap() {
                EndpointDelta::Remove(addr) => {
                    assert_eq!(addr.to_string(), "10.0.0.1:8080");
                    saw_remove = true;
                }
                EndpointDelta::Insert(addr) => {
                    assert_eq!(addr.to_string(), "10.0.0.2:8080");
                    saw_insert = true;
                }
            }
        }
        assert!(saw_remove, "should have removed old endpoint");
        assert!(saw_insert, "should have inserted new endpoint");
    }
}
