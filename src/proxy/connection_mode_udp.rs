use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::net::UdpPacket;
use crate::net::quic::MAX_DATAGRAM_SIZE;

const TIMEOUT: u64 = 60000; // in milliseconds

pub struct Config {
    pub local_proxy_addr: scion_proto::address::SocketAddr,
    pub remote_client_addr: scion_proto::address::SocketAddr,
    pub mqtt_broker_address: std::net::SocketAddr,
}

pub struct Connection {
    pub config: Config,
    udp_socket: Option<tokio::net::UdpSocket>,
    rx_udp_to_quic: mpsc::Receiver<UdpPacket>,
    tx_quic_to_udp: mpsc::Sender<UdpPacket>,
    cancel_token: CancellationToken,
}

impl Connection {
    pub fn new(
        config: Config,
        rx_udp_to_quic: mpsc::Receiver<UdpPacket>,
        tx_quic_to_udp: mpsc::Sender<UdpPacket>,
        cancel_token: CancellationToken,
    ) -> Self {
        Connection {
            config,
            udp_socket: None,
            rx_udp_to_quic,
            tx_quic_to_udp,
            cancel_token,
        }
    }

    pub async fn handle_client_connection(&mut self) -> Result<()> {
        let mut buf = [0; MAX_DATAGRAM_SIZE];

        let udp_socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        self.udp_socket = Some(udp_socket);

        loop {
            let timeout = std::time::Duration::from_millis(TIMEOUT);
            tokio::select! {
                // Handle connection timeout
                _ = tokio::time::sleep(timeout) => {
                    info!("connection timed out");
                    break;
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
                    if packet.src != self.config.remote_client_addr {
                        info!(
                            "received packet from new source: {:?}",
                            packet.src
                        );
                        self.config.remote_client_addr = packet.src;
                    }

                    self.udp_socket.as_ref().unwrap().send_to(
                        &packet.data,
                        self.config.mqtt_broker_address,
                    ).await?;
                    debug!(
                        "forwarded {} bytes to broker at {:?}",
                        packet.data.len(),
                        self.config.mqtt_broker_address
                    );
                }

                // Handle data from UDP socket
                result = self.udp_socket.as_ref().unwrap().recv_from(&mut buf) => {
                    let (size, src_addr) = match result {
                        Ok(res) => res,
                        Err(e) => {
                            error!("failed to receive from UDP socket: {:?}", e);
                            continue;
                        }
                    };

                    debug!(
                        "received {} bytes from broker at {:?}",
                        size,
                        src_addr
                    );

                    let packet = UdpPacket {
                        data: buf[..size].to_vec(),
                        src: self.config.local_proxy_addr,
                        dst: self.config.remote_client_addr,
                    };

                    self.tx_quic_to_udp.send(packet).await?;
                }


                _ = self.cancel_token.cancelled() => {
                    info!("cancellation requested, shutting down connection handler");
                    break;
                }
            }
        }

        Ok(())
    }
}
