//! Build script for xds-client to compile protobuf definitions

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Only compile the specific proto files we actually need
    // Note: google.protobuf.* types are provided by prost-types automatically
    let proto_files = &[
        // Core discovery service
        "proto/envoy/service/discovery/v3/ads.proto",
        "proto/envoy/service/discovery/v3/discovery.proto",
        // Config resources
        "proto/envoy/config/listener/v3/listener.proto",
        "proto/envoy/config/listener/v3/api_listener.proto",
        "proto/envoy/config/listener/v3/listener_components.proto",
        "proto/envoy/config/listener/v3/udp_listener_config.proto",
        "proto/envoy/config/route/v3/route.proto",
        "proto/envoy/config/route/v3/route_components.proto",
        "proto/envoy/config/route/v3/scoped_route.proto",
        "proto/envoy/config/cluster/v3/cluster.proto",
        "proto/envoy/config/cluster/v3/circuit_breaker.proto",
        "proto/envoy/config/cluster/v3/filter.proto",
        "proto/envoy/config/cluster/v3/outlier_detection.proto",
        "proto/envoy/config/endpoint/v3/endpoint.proto",
        "proto/envoy/config/endpoint/v3/endpoint_components.proto",
        // Config common
        "proto/envoy/config/common/mutation_rules/v3/mutation_rules.proto",
        // Config accesslog
        "proto/envoy/config/accesslog/v3/accesslog.proto",
        // Config trace
        "proto/envoy/config/trace/v3/http_tracer.proto",
        // Config core - all files needed
        "proto/envoy/config/core/v3/base.proto",
        "proto/envoy/config/core/v3/address.proto",
        "proto/envoy/config/core/v3/backoff.proto",
        "proto/envoy/config/core/v3/http_uri.proto",
        "proto/envoy/config/core/v3/protocol.proto",
        "proto/envoy/config/core/v3/config_source.proto",
        "proto/envoy/config/core/v3/grpc_service.proto",
        "proto/envoy/config/core/v3/extension.proto",
        "proto/envoy/config/core/v3/socket_option.proto",
        "proto/envoy/config/core/v3/substitution_format_string.proto",
        "proto/envoy/config/core/v3/health_check.proto",
        "proto/envoy/config/core/v3/resolver.proto",
        "proto/envoy/config/core/v3/proxy_protocol.proto",
        // HTTP Connection Manager
        "proto/envoy/extensions/filters/network/http_connection_manager/v3/http_connection_manager.proto",
        // Type matcher - compile all files in this package
        "proto/envoy/type/matcher/v3/regex.proto",
        "proto/envoy/type/matcher/v3/string.proto",
        "proto/envoy/type/matcher/v3/metadata.proto",
        "proto/envoy/type/matcher/v3/node.proto",
        "proto/envoy/type/matcher/v3/number.proto",
        "proto/envoy/type/matcher/v3/path.proto",
        "proto/envoy/type/matcher/v3/value.proto",
        "proto/envoy/type/matcher/v3/struct.proto",
        "proto/envoy/type/matcher/v3/http_inputs.proto",
        "proto/envoy/type/matcher/v3/address.proto",
        "proto/envoy/type/matcher/v3/filter_state.proto",
        "proto/envoy/type/matcher/v3/status_code_input.proto",
        // Type metadata
        "proto/envoy/type/metadata/v3/metadata.proto",
        // Type tracing
        "proto/envoy/type/tracing/v3/custom_tag.proto",
        // Type HTTP
        "proto/envoy/type/http/v3/path_transformation.proto",
        // Type v3
        "proto/envoy/type/v3/http.proto",
        "proto/envoy/type/v3/percent.proto",
        "proto/envoy/type/v3/range.proto",
        "proto/envoy/type/v3/semantic_version.proto",
        "proto/envoy/type/v3/http_status.proto",
        "proto/envoy/type/v3/ratelimit_unit.proto",
        "proto/envoy/type/v3/ratelimit_strategy.proto",
        "proto/envoy/type/v3/token_bucket.proto",
        "proto/envoy/type/v3/hash_policy.proto",
        // xDS Core
        "proto/xds/core/v3/resource_name.proto",
        "proto/xds/core/v3/resource_locator.proto",
        "proto/xds/core/v3/context_params.proto",
        "proto/xds/core/v3/authority.proto",
        "proto/xds/core/v3/cidr.proto",
        "proto/xds/core/v3/collection_entry.proto",
        "proto/xds/core/v3/resource.proto",
        "proto/xds/core/v3/extension.proto",
        // xDS Type Matcher
        "proto/xds/type/matcher/v3/matcher.proto",
        "proto/xds/type/matcher/v3/regex.proto",
        "proto/xds/type/matcher/v3/string.proto",
        "proto/xds/type/matcher/v3/http_inputs.proto",
        // Google RPC
        "proto/google/rpc/status.proto",
        "proto/google/rpc/code.proto",
        "proto/google/rpc/error_details.proto",
        // Envoy annotations
        "proto/envoy/annotations/deprecation.proto",
        // UDPA annotations (needed for many envoy protos)
        "proto/udpa/annotations/status.proto",
        "proto/udpa/annotations/versioning.proto",
        "proto/udpa/annotations/migrate.proto",
        "proto/udpa/annotations/security.proto",
        // Validate (needed for many envoy protos)
        "proto/validate/validate.proto",
    ];

    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(
            proto_files,
            &["proto"],
        )?;
    Ok(())
}
