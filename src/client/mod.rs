use std::fs;

use anyhow::{Context, Result, anyhow};
use mqtt_protocol_core::mqtt;
use mqtt_protocol_core::mqtt::prelude::*;
use ring::rand::{SecureRandom, SystemRandom};
use scion_proto::path::policy::acl::AclPolicy;
use scion_stack::scionstack::ScionStackBuilder;
use tracing::{debug, error, info, trace, warn};

use crate::net::quic::MAX_DATAGRAM_SIZE;
use crate::net::quic::{DEFAULT_TIMEOUT, KEEPALIVE_INTERVAL};

const PUBLISH_INTERVAL: u64 = 5000; // in milliseconds

pub const UDP_PACKET_BUFFER_SIZE: usize = 65535;

pub struct Connection {
    conn: quiche::Connection,
    mqtt_conn: mqtt::GenericConnection<mqtt::role::Client, u16>,
    mqtt_init_sent: bool,
}

#[derive(Clone)]
pub struct ClientConfig {
    pub bind: Option<scion_proto::address::SocketAddr>,
    pub remote: scion_proto::address::SocketAddr,
    pub host: Option<String>,
    pub endhost_api_address: Option<url::Url>,
    pub snap_token_path: Option<std::path::PathBuf>,
    pub acl_policy: Option<AclPolicy>,
    pub ca_cert_path: std::path::PathBuf,
    pub cert_path: std::path::PathBuf,
    pub key_path: std::path::PathBuf,
}

pub struct Client {
    config: ClientConfig,
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        Client { config }
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut quic_config = crate::net::quic::configure_quic(
            &self.config.ca_cert_path,
            &self.config.cert_path,
            &self.config.key_path,
        )?;

        let endhost_api_url = self.config.endhost_api_address.clone().context(
            "endhost API address must be provided when using SCION (with --endhost-api)",
        )?;

        let mut builder = ScionStackBuilder::new(endhost_api_url);
        if let Some(token_path) = &self.config.snap_token_path {
            let snap_token = fs::read_to_string(token_path)
                .with_context(|| format!("failed to read token file {:?}", token_path))?
                .trim()
                .to_string();
            if snap_token.is_empty() {
                anyhow::bail!("token file {:?} is empty", token_path);
            }
            builder = builder.with_auth_token(snap_token);
        }
        let client_network_stack = builder.build().await?;

        let local_ias = client_network_stack.local_ases();
        if let Some(bind) = &self.config.bind
            && !local_ias.contains(&bind.isd_asn())
        {
            anyhow::bail!(
                "configured bind ISD-AS {} is not among the local ASes of the SCION stack: {:?}",
                bind.isd_asn(),
                local_ias
            );
        }

        let mut socket_config = scion_stack::scionstack::SocketConfig::new();
        if let Some(policy) = &self.config.acl_policy {
            info!("Using ACL policy: {:?}", policy);
            socket_config = socket_config.with_path_policy(policy.clone());
        } else {
            info!("No ACL policy specified, using default (allow all)");
        }

        let socket = client_network_stack
            .bind_with_config(self.config.bind, socket_config)
            .await?;

        // Get local address.
        let local_addr = socket.local_addr();

        // Generate a random source connection ID for the connection.
        let mut scid = [0; quiche::MAX_CONN_ID_LEN];
        if let Err(e) = SystemRandom::new().fill(&mut scid[..]) {
            return Err(anyhow::anyhow!("failed to generate scid: {:?}", e));
        }
        let scid = quiche::ConnectionId::from_ref(&scid);

        // Create a QUIC connection and initiate handshake.
        let conn = quiche::connect(
            self.config.host.as_deref(),
            &scid,
            local_addr.local_address().ok_or(anyhow::anyhow!(
                "failed to get local address from {:?}",
                local_addr
            ))?,
            self.config.remote.local_address().ok_or(anyhow::anyhow!(
                "failed to get remote address from {:?}",
                self.config.remote
            ))?,
            &mut quic_config,
        )?;

        let mut mqtt_conn = mqtt::Connection::<mqtt::role::Client>::new(mqtt::Version::V5_0);
        mqtt_conn.set_auto_pub_response(true);

        let mut connection = Connection {
            conn,
            mqtt_conn,
            mqtt_init_sent: false,
        };

        let mut buf = [0; UDP_PACKET_BUFFER_SIZE];
        let mut send_buf = [0; MAX_DATAGRAM_SIZE];

        // Send initial packet
        let (write, send_info) = connection.conn.send(&mut send_buf)?;
        if let Err(e) = socket
            .send_to(
                &send_buf[..write],
                scion_proto::address::SocketAddr::from_std(
                    self.config.remote.isd_asn(),
                    send_info.to,
                ),
            )
            .await
        {
            error!("initial send_to failed to write {} bytes: {:?}", write, e);
            return Err(anyhow!("initial send failed: {:?}", e));
        }

        let mut publish_interval =
            tokio::time::interval(std::time::Duration::from_millis(PUBLISH_INTERVAL));
        let mut keepalive_interval =
            tokio::time::interval(std::time::Duration::from_millis(KEEPALIVE_INTERVAL));

        // Main loop
        loop {
            let timeout = connection
                .conn
                .timeout()
                .unwrap_or(std::time::Duration::from_millis(DEFAULT_TIMEOUT));

            tokio::select! {
                // Connection timeout
                _ = tokio::time::sleep(timeout) => {
                    debug!("connection timeout");
                    connection.conn.on_timeout();
                }

                // Connection keepalive
                _ = keepalive_interval.tick() => {
                    if connection.conn.is_established() {
                        connection.conn.send_ack_eliciting()?;
                        trace!("keepalive tick. time until timeout: {:?}", connection.conn.timeout());
                    }
                }

                // Receive datagram from UDP socket and pass to QUIC
                Ok((len, src)) = socket.recv_from(&mut buf) => {
                    debug!("received {} bytes on socket from {}", len, src);
                    self.process_udp_packet(&mut connection, &mut buf[..len], src, local_addr).await?;
                }

                // Publish interval
                _ = publish_interval.tick() => {
                    if connection.conn.is_established() {
                        // Build PUBLISH packet
                        let mut publish_builder = mqtt::packet::v5_0::Publish::builder()
                            .topic_name("test/topic")
                            .unwrap()
                            .payload(b"Hello from MQTT over QUIC over SCION!".to_vec())
                            .qos(mqtt::packet::Qos::AtLeastOnce);

                        let packet_id = connection.mqtt_conn.acquire_packet_id()
                            .map_err(|_| anyhow!("no packet ID available for PUBLISH packet"))?;
                        publish_builder = publish_builder.packet_id(packet_id);

                        let publish_packet = publish_builder.build()
                            .map_err(|e| anyhow!("failed to build PUBLISH packet: {}", e))?;

                        // Send through connection (returns events to handle)
                        let events = connection.mqtt_conn.checked_send(publish_packet);
                        self.handle_events(&mut connection, events)
                            .map_err(|e| anyhow!("failed to handle events: {}", e))?;

                        info!("sent PUBLISH packet on stream {}", 0);
                    }
                }
            }

            // Check if connection is closed
            if connection.conn.is_closed() {
                info!("connection closed");
                if connection.conn.is_timed_out() {
                    warn!("connection hit local idle-timeout");
                }
                if let Some(err) = connection.conn.peer_error() {
                    warn!(
                        "peer closed connection (is_app={}, code={}, reason={:?})",
                        err.is_app,
                        err.error_code,
                        String::from_utf8_lossy(&err.reason)
                    );
                }
                debug!("connection stats, {:?}", connection.conn.stats());
                break;
            }

            // Send any pending QUIC packets
            loop {
                let (write, send_info) = match connection.conn.send(&mut send_buf) {
                    Ok(v) => v,
                    Err(quiche::Error::Done) => break,
                    Err(e) => {
                        error!("send failed: {:?}", e);
                        break;
                    }
                };

                if let Err(e) = socket
                    .send_to(
                        &send_buf[..write],
                        scion_proto::address::SocketAddr::from_std(
                            self.config.remote.isd_asn(),
                            send_info.to,
                        ),
                    )
                    .await
                {
                    error!("send_to failed to write {} bytes: {:?}", write, e);
                    return Err(anyhow!("send failed: {:?}", e));
                }
            }
        }
        Ok(())
    }

    async fn process_udp_packet(
        &mut self,
        connection: &mut Connection,
        data: &mut [u8],
        src: scion_proto::address::SocketAddr,
        local_addr: scion_proto::address::SocketAddr,
    ) -> Result<()> {
        let mut buf = [0; MAX_DATAGRAM_SIZE];
        let recv_info = quiche::RecvInfo {
            from: src
                .local_address()
                .ok_or_else(|| anyhow!("invalid src address"))?,
            to: local_addr
                .local_address()
                .ok_or_else(|| anyhow!("invalid dst address"))?,
        };

        connection.conn.recv(data, recv_info)?;

        if connection.conn.is_established() && !connection.mqtt_init_sent {
            // Send MQTT CONNECT packet
            // Build CONNECT packet
            let connect_packet = mqtt::packet::v5_0::Connect::builder()
                .client_id("my_client")
                .unwrap()
                .build()
                .map_err(|e| anyhow!("failed to build CONNECT packet: {}", e))?;

            // Send through connection (returns events to handle)
            let events = connection.mqtt_conn.checked_send(connect_packet);
            self.handle_events(connection, events)
                .map_err(|e| anyhow!("failed to handle events: {}", e))?;

            connection.mqtt_init_sent = true;
            info!("sent MQTT CONNECT packet on stream {}", 0);
        }

        if connection.conn.is_established() {
            // Iterate over readable streams.
            for stream_id in connection.conn.readable() {
                if stream_id != 0 {
                    warn!("received data on unknown stream {}", stream_id);
                    continue;
                }
                // Stream is readable, read until there's no more data.
                while let Ok((read, _)) = connection.conn.stream_recv(stream_id, &mut buf) {
                    debug!("Got {} bytes on stream {}", read, stream_id);
                    if stream_id == 0 {
                        // Handle data on stream 0 (MQTT stream)
                        let mut cursor = mqtt::common::Cursor::new(&buf[..read]);

                        let events = connection.mqtt_conn.recv(&mut cursor);
                        self.handle_events(connection, events)
                            .map_err(|e| anyhow!("failed to handle events: {}", e))?;
                    }
                }
            }
        }

        Ok(())
    }

    fn handle_events(
        &mut self,
        connection: &mut Connection,
        events: Vec<mqtt::connection::Event>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for event in events {
            match event {
                mqtt::connection::Event::RequestSendPacket { packet, .. } => {
                    let buffer = packet.to_continuous_buffer();
                    match connection.conn.stream_send(0, &buffer, false) {
                        Ok(_) => {}
                        Err(e) => {
                            error!("Failed to send packet on stream: {:?}", e);
                        }
                    }
                    let packet_type = packet.packet_type();
                    info!("Sent packet: {packet_type}");
                }
                mqtt::connection::Event::NotifyPacketReceived(packet) => match packet {
                    mqtt::packet::Packet::V5_0Connack(connack) => {
                        let reason_code = connack.reason_code();
                        info!("CONNACK received: {reason_code:?}");
                    }
                    mqtt::packet::Packet::V5_0Puback(puback) => {
                        let packet_id = puback.packet_id();
                        info!("PUBACK received for packet ID: {packet_id}");
                    }
                    mqtt::packet::Packet::V5_0Pubrec(pubrec) => {
                        let packet_id = pubrec.packet_id();
                        info!("PUBREC received for packet ID: {packet_id}");
                    }
                    mqtt::packet::Packet::V5_0Pubcomp(pubcomp) => {
                        let packet_id = pubcomp.packet_id();
                        info!("PUBCOMP received for packet ID: {packet_id}");
                    }
                    _ => {
                        let packet_type = packet.packet_type();
                        info!("Received packet: {packet_type}");
                    }
                },
                mqtt::connection::Event::NotifyPacketIdReleased(packet_id) => {
                    info!("Packet ID {packet_id} released");
                }
                mqtt::connection::Event::NotifyError(error) => {
                    error!("MQTT Error: {error:?}");
                }
                mqtt::connection::Event::RequestClose => {
                    warn!("Connection close requested");
                    return Ok(());
                }
                mqtt::connection::Event::RequestTimerReset { kind, duration_ms } => {
                    warn!("Timer reset requested: {kind:?} for {duration_ms} ms");
                }
                mqtt::connection::Event::RequestTimerCancel(kind) => {
                    warn!("Timer cancel requested: {kind:?}");
                }
            }
        }
        Ok(())
    }
}
