use anyhow::{Result, anyhow};
use mqtt_protocol_core::mqtt;
use mqtt_protocol_core::mqtt::prelude::*;
use ring::rand::{SecureRandom, SystemRandom};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::client::MAX_DATAGRAM_SIZE;
use crate::net::UdpPacket;
use crate::net::quic::{DEFAULT_TIMEOUT, KEEPALIVE_INTERVAL};

const PUBLISH_INTERVAL: u64 = 5000; // in milliseconds

pub struct Config {
    pub server_name: String,
    pub quic_config: quiche::Config,
    pub local: scion_proto::address::SocketAddr,
    pub remote: scion_proto::address::SocketAddr,
}

pub struct Connection {
    config: Config,
    conn: quiche::Connection,
    mqtt_conn: mqtt::GenericConnection<mqtt::role::Client, u16>,
    rx_udp_to_quic: mpsc::Receiver<UdpPacket>,
    tx_quic_to_udp: mpsc::Sender<UdpPacket>,
    cancel_token: CancellationToken,
    mqtt_init_sent: bool,
}

impl Connection {
    pub fn new(
        mut config: Config,
        rx_udp_to_quic: mpsc::Receiver<UdpPacket>,
        tx_quic_to_udp: mpsc::Sender<UdpPacket>,
        cancel_token: CancellationToken,
    ) -> Result<Self> {
        // Generate a random source connection ID for the connection.
        let mut scid = [0; quiche::MAX_CONN_ID_LEN];
        if let Err(e) = SystemRandom::new().fill(&mut scid[..]) {
            return Err(anyhow::anyhow!("failed to generate scid: {:?}", e));
        }
        let scid = quiche::ConnectionId::from_ref(&scid);

        // Create a QUIC connection and initiate handshake.
        let conn = quiche::connect(
            Some(&config.server_name),
            &scid,
            config.local.local_address().ok_or(anyhow::anyhow!(
                "failed to get local address from {:?}",
                config.local
            ))?,
            config.remote.local_address().ok_or(anyhow::anyhow!(
                "failed to get remote address from {:?}",
                config.remote
            ))?,
            &mut config.quic_config,
        )?;

        info!(
            "connecting to {:} from {:} with scid {:?}",
            config.remote, config.local, scid
        );

        let mut mqtt_conn = mqtt::Connection::<mqtt::role::Client>::new(mqtt::Version::V5_0);
        mqtt_conn.set_auto_pub_response(true);

        Ok(Connection {
            config,
            conn,
            mqtt_conn,
            rx_udp_to_quic,
            tx_quic_to_udp,
            cancel_token,
            mqtt_init_sent: false,
        })
    }

    pub async fn start_connection_handling(&mut self) -> Result<()> {
        let mut buf = [0; MAX_DATAGRAM_SIZE];

        // Send initial packet
        let (write, send_info) = self.conn.send(&mut buf)?;
        self.tx_quic_to_udp
            .send(UdpPacket {
                data: buf[..write].to_vec(),
                src: scion_proto::address::SocketAddr::from_std(
                    self.config.local.isd_asn(),
                    send_info.from,
                ),
                dst: scion_proto::address::SocketAddr::from_std(
                    self.config.remote.isd_asn(),
                    send_info.to,
                ),
            })
            .await?;

        let mut publish_interval =
            tokio::time::interval(std::time::Duration::from_millis(PUBLISH_INTERVAL));
        let mut keepalive_interval =
            tokio::time::interval(std::time::Duration::from_millis(KEEPALIVE_INTERVAL));
        loop {
            let timeout = self
                .conn
                .timeout()
                .unwrap_or(std::time::Duration::from_millis(DEFAULT_TIMEOUT));

            tokio::select! {
                // Connection timeout
                _ = tokio::time::sleep(timeout) => {
                    debug!("connection timeout");
                    self.conn.on_timeout();
                }

                // Connection keepalive
                _ = keepalive_interval.tick() => {
                    if self.conn.is_established() {
                        self.conn.send_ack_eliciting()?;
                        trace!("keepalive tick. time until timeout: {:?}", self.conn.timeout());
                    }
                }

                // Handle incoming UDP packets
                packet = self.rx_udp_to_quic.recv() => {
                    let packet = match packet {
                        Some(pkt) => pkt,
                        None => {
                            warn!("UDP to QUIC channel closed");
                            break;
                        }
                    };
                    self.process_udp_packet(packet).await?;
                }


                _ = publish_interval.tick() => {
                    if self.conn.is_established() {
                        // Build PUBLISH packet
                        let mut publish_builder = mqtt::packet::v5_0::Publish::builder()
                            .topic_name("test/topic")
                            .unwrap()
                            .payload(b"Hello from MQTT over QUIC over SCION!".to_vec())
                            .qos(mqtt::packet::Qos::AtLeastOnce);


                        let packet_id = self.mqtt_conn.acquire_packet_id()
                            .map_err(|_| anyhow!("no packet ID available for PUBLISH packet"))?;
                        publish_builder = publish_builder.packet_id(packet_id);

                        let publish_packet = publish_builder.build()
                            .map_err(|e| anyhow!("failed to build PUBLISH packet: {}", e))?;

                        // Send through connection (returns events to handle)
                        let events = self.mqtt_conn.checked_send(publish_packet);
                        self.handle_events(events)
                            .map_err(|e| anyhow!("failed to handle events: {}", e))?;

                        info!("sent PUBLISH packet on stream {}", 0);
                    }
                }

                _ = self.cancel_token.cancelled() => {
                    info!("cancellation requested, shutting down connection handler");
                    break;
                }
            }

            // Check if connection is closed
            if self.conn.is_closed() {
                info!("connection closed");
                if self.conn.is_timed_out() {
                    warn!("connection hit local idle-timeout");
                }
                if let Some(err) = self.conn.peer_error() {
                    warn!(
                        "peer closed connection (is_app={}, code={}, reason={:?})",
                        err.is_app,
                        err.error_code,
                        String::from_utf8_lossy(&err.reason)
                    );
                }
                debug!("connection stats, {:?}", self.conn.stats());
                break;
            }

            // Send any pending QUIC packets
            loop {
                let (write, send_info) = match self.conn.send(&mut buf) {
                    Ok(v) => v,
                    Err(quiche::Error::Done) => break,
                    Err(e) => {
                        error!("send failed: {:?}", e);
                        break;
                    }
                };

                if self
                    .tx_quic_to_udp
                    .send(UdpPacket {
                        data: buf[..write].to_vec(),
                        src: scion_proto::address::SocketAddr::from_std(
                            self.config.local.isd_asn(),
                            send_info.from,
                        ),
                        dst: scion_proto::address::SocketAddr::from_std(
                            self.config.remote.isd_asn(),
                            send_info.to,
                        ),
                    })
                    .await
                    .is_err()
                {
                    info!("UDP channel closed, cannot send packets");
                    break;
                }
            }
        }

        info!("QUIC connection handler exiting");
        Ok(())
    }

    async fn process_udp_packet(&mut self, mut packet: UdpPacket) -> Result<()> {
        let mut buf = [0; MAX_DATAGRAM_SIZE];
        let recv_info = quiche::RecvInfo {
            from: packet
                .src
                .local_address()
                .ok_or_else(|| anyhow!("invalid src address"))?,
            to: packet
                .dst
                .local_address()
                .ok_or_else(|| anyhow!("invalid dst address"))?,
        };

        self.conn.recv(&mut packet.data, recv_info)?;

        if self.conn.is_established() && !self.mqtt_init_sent {
            // Send MQTT CONNECT packet
            // Build CONNECT packet
            let connect_packet = mqtt::packet::v5_0::Connect::builder()
                .client_id("my_client")
                .unwrap()
                .build()
                .map_err(|e| anyhow!("failed to build CONNECT packet: {}", e))?;

            // Send through connection (returns events to handle)
            let events = self.mqtt_conn.checked_send(connect_packet);
            self.handle_events(events)
                .map_err(|e| anyhow!("failed to handle events: {}", e))?;

            self.mqtt_init_sent = true;
            info!("sent MQTT CONNECT packet on stream {}", 0);
        }

        if self.conn.is_established() {
            // Iterate over readable streams.
            for stream_id in self.conn.readable() {
                if stream_id != 0 {
                    warn!("received data on unknown stream {}", stream_id);
                    continue;
                }
                // Stream is readable, read until there's no more data.
                while let Ok((read, _)) = self.conn.stream_recv(stream_id, &mut buf) {
                    debug!("Got {} bytes on stream {}", read, stream_id);
                    if stream_id == 0 {
                        // Handle data on stream 0 (MQTT stream)
                        let mut cursor = mqtt::common::Cursor::new(&buf[..read]);

                        let events = self.mqtt_conn.recv(&mut cursor);
                        self.handle_events(events)
                            .map_err(|e| anyhow!("failed to handle events: {}", e))?;
                    }
                }
            }
        }

        Ok(())
    }

    fn handle_events(
        &mut self,
        events: Vec<mqtt::connection::Event>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for event in events {
            match event {
                mqtt::connection::Event::RequestSendPacket { packet, .. } => {
                    let buffer = packet.to_continuous_buffer();
                    match self.conn.stream_send(0, &buffer, false) {
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
