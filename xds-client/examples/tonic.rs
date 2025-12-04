//! Proof of concept for the XDS client using Tonic.

use std::time::Duration;
use tokio_stream::StreamExt;
use xds_client::{
    client::{builder::XdsClientBuilder, config::{ClientConfig, TlsConfig}, watch::XdsUpdate},
    resource::{ListenerResource, RouteResource},
    runtime::tokio::TokioRuntime,
    transport::tonic::TonicTransportFactory
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    println!("Connecting to xDS server...");

    let ca_cert_path = std::env::var("CA_CERT_PATH")
        .expect("CA_CERT_PATH must be set");
    let client_cert_path = std::env::var("CLIENT_CERT_PATH")
        .expect("CLIENT_CERT_PATH must be set");
    let client_key_path = std::env::var("CLIENT_KEY_PATH")
        .expect("CLIENT_KEY_PATH must be set");

    let tls_config = TlsConfig {
        ca_cert_pem: std::fs::read(&ca_cert_path).ok(),
        client_cert_pem: std::fs::read(&client_cert_path).ok(),
        client_key_pem: std::fs::read(&client_key_path).ok(),
        domain_name: Some(std::env::var("XDS_DOMAIN_NAME").expect("XDS_DOMAIN_NAME must be set")),
    };

    let server_uri = std::env::var("XDS_SERVER_URI")
        .expect("XDS_SERVER_URI must be set");
    let node_id = std::env::var("XDS_NODE_ID")
        .expect("XDS_NODE_ID must be set");

    let config = ClientConfig {
        server_uri,
        node_id,
        tls_config: Some(tls_config),
        connect_timeout: Duration::from_secs(5),
    };

    let builder = XdsClientBuilder::new(config.clone());
    let runtime = TokioRuntime;
    let transport = TonicTransportFactory::new(&config).await?;

    let client = builder.build_with(runtime, transport).await?;

    let resource_name = std::env::var("XDS_RESOURCE_NAME")
        .expect("XDS_RESOURCE_NAME must be set");

    println!("Connected. Watching for listener '{}'...", resource_name);
    let mut watcher = client
        .watch::<ListenerResource>(resource_name)
        .await?;

    println!("Waiting for updates (Press Ctrl+C to exit)...");
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("Ctrl-C received, exiting.");
                break;
            }
            update = watcher.next() => {
                match update {
                    Some(XdsUpdate::Set(listener)) => {
                        println!("Received update: Listener(name={})", listener.name);
                        
                        if let Some(route_name) = listener.route_config_name {
                            println!("Found route config name: {}", route_name);
                            
                            println!("Watching for route '{}'...", route_name);
                            let mut route_watcher = client.watch::<RouteResource>(route_name).await?;
                            
                            // Spawn a task to handle route updates
                            tokio::spawn(async move {
                                while let Some(update) = route_watcher.next().await {
                                    match update {
                                        XdsUpdate::Set(route) => {
                                            println!("Received update: RouteConfiguration(name={})", route.name);
                                            for vh in route.virtual_hosts {
                                                println!("  VirtualHost: {} (domains: {:?})", vh.name, vh.domains);
                                            }
                                        }
                                        XdsUpdate::Error(e) => println!("Route Watch Error: {:?}", e),
                                        XdsUpdate::Remove => println!("Route Removed"),
                                    }
                                }
                            });
                        }
                    }
                    Some(XdsUpdate::Error(error)) => {
                        println!("Error: {:?}", error);
                    }
                    Some(XdsUpdate::Remove) => {
                        println!("Resource removed");
                    }
                    None => {
                        println!("Watcher closed");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}
