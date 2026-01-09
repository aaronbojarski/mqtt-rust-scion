use std::path::PathBuf;

use anyhow::anyhow;
use clap::{Args, Parser, Subcommand};
use scion_proto::path::policy::acl::AclPolicy;
use url::Url;

#[derive(Parser, Debug)]
#[clap(
    name = "mqtt-rust-scion",
    about = "Run the MQTT Rust SCION proxy or client",
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Proxy(ProxyOpt),
    Client(ClientOpt),
}

#[derive(Args, Debug)]
struct ProxyOpt {
    /// Address to listen on
    #[clap(long)]
    listen: scion_proto::address::SocketAddr,

    /// Address of the endhost API to connect to for scion path resolution. Required when using SCION.
    #[clap(long = "endhost-api")]
    endhost_api_address: Option<Url>,

    /// Path to the Snap token file for authentication with the endhost API
    #[clap(long = "snap-token", value_name = "FILE")]
    snap_token_path: Option<PathBuf>,

    /// ACL policy to filter SCION paths
    #[clap(long = "acl", allow_hyphen_values = true)]
    acl: Option<AclPolicy>,

    /// CA certificate used to verify peers
    #[clap(long = "ca-cert", value_name = "FILE", default_value = "ca-cert.pem")]
    ca_cert_path: PathBuf,

    /// Certificate presented to clients
    #[clap(long = "cert", value_name = "FILE", default_value = "proxy-cert.pem")]
    cert_path: PathBuf,

    /// Private key for the presented certificate
    #[clap(long = "key", value_name = "FILE", default_value = "proxy-key.pem")]
    key_path: PathBuf,

    /// Proxy operation mode: QuicEndpoint or UdpEndpoint
    #[clap(long = "mode", default_value = "QuicEndpoint")]
    mode: mqtt_rust_scion::proxy::Mode,

    /// Address of the MQTT broker to forward traffic to
    #[clap(long = "mqtt-broker", default_value = "127.0.0.1:1883")]
    mqtt_broker_address: core::net::SocketAddr,

    /// Tracing level (trace, debug, info, warn, error)
    #[clap(long = "log", default_value = "info")]
    log_level: tracing::Level,
}

#[derive(Args, Debug)]
struct ClientOpt {
    /// Address of proxy to connect to (e.g. [1-2,3.4.5.6]:4433)
    remote: scion_proto::address::SocketAddr,

    /// Hostname used for certificate verification
    #[clap(long = "host")]
    host: Option<String>,

    /// Local address to bind to
    #[clap(long = "bind")]
    bind: Option<scion_proto::address::SocketAddr>,

    /// Address of the endhost API to connect to for scion path resolution. Required when using SCION.
    #[clap(long = "endhost-api")]
    endhost_api_address: Url,

    /// Path to the Snap token file for authentication with the endhost API
    #[clap(long = "snap-token", value_name = "FILE")]
    snap_token_path: Option<PathBuf>,

    /// ACL policy to filter SCION paths
    #[clap(long = "acl", allow_hyphen_values = true)]
    acl: Option<AclPolicy>,

    /// CA certificate used to verify the proxy
    #[clap(long = "ca-cert", value_name = "FILE", default_value = "ca-cert.pem")]
    ca_cert_path: PathBuf,

    /// Client certificate presented to the proxy
    #[clap(long = "cert", value_name = "FILE", default_value = "client-cert.pem")]
    cert_path: PathBuf,

    /// Private key for the client certificate
    #[clap(long = "key", value_name = "FILE", default_value = "client-key.pem")]
    key_path: PathBuf,

    /// Client identifier
    #[clap(long = "client-id", default_value = "mqtt-rust-scion-client")]
    client_id: String,

    /// Tracing level (trace, debug, info, warn, error)
    #[clap(long = "log", default_value = "info")]
    log_level: tracing::Level,

    #[clap(subcommand)]
    command: ClientCommand,
}

#[derive(Subcommand, Debug)]
enum ClientCommand {
    Publish(PublishOpt),
    Subscribe(SubscribeOpt),
}

#[derive(Args, Debug)]
struct PublishOpt {
    /// Topic to publish to
    topic: String,

    /// Payload to publish
    payload: String,
}

#[derive(Args, Debug)]
struct SubscribeOpt {
    /// Topic to subscribe to
    topic: String,
}

fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Proxy(opt) => run_proxy(opt),
        Command::Client(opt) => run_client(opt),
    };
    if let Err(ref err) = result {
        tracing::error!(error = %err, "command failed");
    }
    result
}

#[tokio::main]
async fn run_proxy(opt: ProxyOpt) -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt()
        .with_max_level(opt.log_level)
        .try_init()
        .map_err(|err| anyhow!("failed to init tracing: {err}"))?;
    let config = mqtt_rust_scion::proxy::ProxyConfig {
        listen: opt.listen,
        endhost_api_address: opt.endhost_api_address,
        snap_token_path: opt.snap_token_path,
        acl_policy: opt.acl,
        ca_cert_path: opt.ca_cert_path,
        cert_path: opt.cert_path,
        key_path: opt.key_path,
        mode: opt.mode,
        mqtt_broker_address: opt.mqtt_broker_address,
    };
    let mut proxy = mqtt_rust_scion::proxy::Proxy::new(config)?;
    proxy.run().await?;
    Ok(())
}

#[tokio::main]
async fn run_client(opt: ClientOpt) -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt()
        .with_max_level(opt.log_level)
        .try_init()
        .map_err(|err| anyhow!("failed to init tracing: {err}"))?;
    let config = mqtt_rust_scion::client::ClientConfig {
        bind: opt.bind,
        host: opt.host,
        endhost_api_address: opt.endhost_api_address,
        snap_token_path: opt.snap_token_path,
        acl_policy: opt.acl,
        ca_cert_path: opt.ca_cert_path,
        cert_path: opt.cert_path,
        key_path: opt.key_path,
        client_id: opt.client_id,
    };

    let mut client = mqtt_rust_scion::client::Client::new(config);
    client.connect(opt.remote).await?;

    match opt.command {
        ClientCommand::Publish(publish_opt) => {
            client
                .publish(&publish_opt.topic, publish_opt.payload.as_bytes().to_vec())
                .await?;
            tracing::info!("Published message to topic '{}'", publish_opt.topic);
        }
        ClientCommand::Subscribe(subscribe_opt) => {
            client.subscribe(&subscribe_opt.topic).await?;
            tracing::info!(
                "Subscribed to topic '{}'. Waiting for messages...",
                subscribe_opt.topic
            );
            while let Some(message) = client.rcv().await.ok() {
                tracing::info!(
                    "Received message: {:?} for topic '{}'",
                    String::from_utf8_lossy(&message.payload),
                    message.topic
                );
            }
        }
    }
    Ok(())
}
