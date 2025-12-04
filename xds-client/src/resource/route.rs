//! Resource for a route configuration.
use crate::resource::XdsResource;
use crate::pb::envoy::config::route::v3::RouteConfiguration as ProtoRouteConfiguration;
use prost::Message;
use crate::error::Result;

/// Route configuration resource.
#[derive(Clone, Debug)]
pub struct RouteConfiguration {
    /// The name of the route configuration.
    pub name: String,
    /// The virtual hosts of the route configuration.
    pub virtual_hosts: Vec<VirtualHost>,
}

/// Virtual host for a route configuration.
#[derive(Clone, Debug)]
pub struct VirtualHost {
    /// The name of the virtual host.
    pub name: String,
    /// The domains of the virtual host.
    pub domains: Vec<String>,
}

/// Resource definition for RouteConfiguration.
#[derive(Debug)]
pub struct RouteResource;

impl XdsResource for RouteResource {
    type Resource = RouteConfiguration;

    fn type_url() -> &'static str {
        "type.googleapis.com/envoy.config.route.v3.RouteConfiguration"
    }

    fn decode(data: &[u8]) -> Result<Self::Resource> {
        let proto = ProtoRouteConfiguration::decode(data)?;
        
        let virtual_hosts = proto.virtual_hosts.into_iter().map(|vh| VirtualHost {
            name: vh.name,
            domains: vh.domains,
        }).collect();

        Ok(RouteConfiguration {
            name: proto.name,
            virtual_hosts,
        })
    }
}

