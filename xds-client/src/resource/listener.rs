//! Resource for a listener.
use crate::resource::XdsResource;
use crate::pb::envoy::config::listener::v3::Listener as ProtoListener;
use crate::pb::envoy::config::listener::v3::filter::ConfigType;
use crate::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_connection_manager::RouteSpecifier;
use crate::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpConnectionManager;
use prost::Message;
use crate::error::Result;

/// Listener resource.
#[derive(Clone, Debug)]
pub struct Listener {
    /// The name of the listener.
    pub name: String,
    /// The name of the route configuration found in RDS, if any.
    pub route_config_name: Option<String>,
}

/// Resource for a listener.
#[derive(Debug)]
pub struct ListenerResource;

impl XdsResource for ListenerResource {
    type Resource = Listener;

    fn type_url() -> &'static str {
        "type.googleapis.com/envoy.config.listener.v3.Listener"
    }

    fn decode(data: &[u8]) -> Result<Self::Resource> {
        let proto = ProtoListener::decode(data)?;
        
        let mut route_config_name = None;

        // 1. Check api_listener (common for gRPC clients)
        if let Some(api_listener) = proto.api_listener {
             if let Some(any) = api_listener.api_listener {
                 if any.type_url == "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager" {
                     let hcm = HttpConnectionManager::decode(&any.value[..])?;
                     if let Some(RouteSpecifier::Rds(rds)) = hcm.route_specifier {
                         route_config_name = Some(rds.route_config_name);
                     }
                 }
             }
        }
        
        // 2. If not found, you might want to check filter_chains (common for Envoy proxies)
        if route_config_name.is_none() {
            for filter_chain in proto.filter_chains {
                for filter in filter_chain.filters {
                     if filter.name == "envoy.filters.network.http_connection_manager" {
                        if let Some(ConfigType::TypedConfig(any)) = filter.config_type {
                            if any.type_url == "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager" {
                                 let hcm = HttpConnectionManager::decode(&any.value[..])?;
                                 if let Some(RouteSpecifier::Rds(rds)) = hcm.route_specifier {
                                     route_config_name = Some(rds.route_config_name);
                                     break;
                                 }
                            }
                        }
                     }
                }
                if route_config_name.is_some() {
                    break;
                }
            }
        }

        Ok(Listener {
            name: proto.name,
            route_config_name,
        })
    }
}
