use anyhow::{Result, anyhow};
use scion_proto::address::IsdAsn;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::net::UdpPacket;
use crate::net::quic::{DEFAULT_TIMEOUT, KEEPALIVE_INTERVAL, MAX_DATAGRAM_SIZE};

const RCV_MANY_CAPACITY: usize = 10; // number of packets to receive at once

pub struct Config {
    pub local_isd_as: IsdAsn,
    pub remote_isd_as: IsdAsn,
}

pub struct Connection {
    pub config: Config,
    conn: quiche::Connection,
    tcp_stream: Option<tokio::net::TcpStream>,
    rx_udp_to_quic: mpsc::Receiver<UdpPacket>,
    tx_quic_to_udp: mpsc::Sender<UdpPacket>,
    cancel_token: CancellationToken,
    client_cert_timer: std::time::Instant,
}

impl Connection {
    pub fn new(
        config: Config,
        conn: quiche::Connection,
        rx_udp_to_quic: mpsc::Receiver<UdpPacket>,
        tx_quic_to_udp: mpsc::Sender<UdpPacket>,
        cancel_token: CancellationToken,
    ) -> Self {
        Connection {
            config,
            conn,
            tcp_stream: None,
            rx_udp_to_quic,
            tx_quic_to_udp,
            cancel_token,
            client_cert_timer: std::time::Instant::now(),
        }
    }

    pub async fn handle_client_connection(&mut self) -> Result<()> {
        let mut buf = [0; MAX_DATAGRAM_SIZE];

        // Send initial response packets
        loop {
            let (write, send_info) = match self.conn.send(&mut buf) {
                Ok(v) => v,
                Err(quiche::Error::Done) => break,
                Err(e) => {
                    error!("send failed: {:?}", e);
                    break;
                }
            };

            let packet = UdpPacket {
                data: buf[..write].to_vec(),
                src: scion_proto::address::SocketAddr::from_std(
                    self.config.local_isd_as,
                    send_info.from,
                ),
                dst: scion_proto::address::SocketAddr::from_std(
                    self.config.remote_isd_as,
                    send_info.to,
                ),
            };

            self.tx_quic_to_udp.send(packet).await?;
        }

        let tcp_stream = tokio::net::TcpStream::connect("127.0.0.1:1883").await?;
        self.tcp_stream = Some(tcp_stream);

        // Create cancellation token for clean shutdown
        let cancel_token = CancellationToken::new();

        let mut udp_packet_buf: Vec<UdpPacket> = Vec::with_capacity(RCV_MANY_CAPACITY); // buffer for incoming UDP packets. Used for processing multiple packets at once.
        let mut keepalive_interval =
            tokio::time::interval(std::time::Duration::from_millis(KEEPALIVE_INTERVAL));

        loop {
            udp_packet_buf.clear();
            let timeout = self
                .conn
                .timeout()
                .unwrap_or(std::time::Duration::from_millis(DEFAULT_TIMEOUT));

            tokio::select! {
                // Handle connection timeout
                _ = tokio::time::sleep(timeout) => {
                    self.conn.on_timeout();
                }

                // Periodic keepalive
                _ = keepalive_interval.tick() => {
                    if self.conn.is_established() {
                        self.conn.send_ack_eliciting()?;
                        trace!("sending keepalive");
                    }
                }

                // Handle incoming UDP packets
                num_packets = self.rx_udp_to_quic.recv_many(&mut udp_packet_buf, RCV_MANY_CAPACITY) => {
                    self.process_udp_packets(&mut udp_packet_buf, num_packets).await?;
                }

                // Handle data from TCP stream
                result = self.tcp_stream.as_mut().unwrap().readable() => {
                    match result {
                        Ok(_) => {
                            let mut tcp_buf = [0; MAX_DATAGRAM_SIZE];
                            match self.tcp_stream.as_mut().unwrap().try_read(&mut tcp_buf) {
                                Ok(0) => {
                                    info!("TCP stream closed by peer");
                                    break;
                                }
                                Ok(n) => {
                                    self.conn.stream_send(0, &tcp_buf[..n], false)?;
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                    // No data available, continue
                                    continue;
                                }
                                Err(e) => {
                                    error!("Failed to read from TCP stream: {}", e);
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            error!("TCP stream error: {}", e);
                            break;
                        }
                    }
                }

                _ = self.cancel_token.cancelled() => {
                    info!("cancellation requested, shutting down connection handler");
                    break;
                }
            }

            // Check if connection was closed while processing packets
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

                self.tx_quic_to_udp
                    .send(UdpPacket {
                        data: buf[..write].to_vec(),
                        src: scion_proto::address::SocketAddr::from_std(
                            self.config.local_isd_as,
                            send_info.from,
                        ),
                        dst: scion_proto::address::SocketAddr::from_std(
                            self.config.remote_isd_as,
                            send_info.to,
                        ),
                    })
                    .await?;
            }
        }

        info!("connection handler exiting, stopping TUN interface",);
        cancel_token.cancel();

        Ok(())
    }

    async fn process_udp_packets(
        &mut self,
        packet_buf: &mut [UdpPacket],
        num_packets: usize,
    ) -> Result<()> {
        for packet in packet_buf.iter_mut().take(num_packets) {
            let src_ip_addr = packet
                .src
                .local_address()
                .ok_or_else(|| anyhow!("Invalid src address."))?;
            let dst_ip_addr = packet
                .dst
                .local_address()
                .ok_or_else(|| anyhow!("Invalid dst address."))?;
            let recv_info = quiche::RecvInfo {
                from: src_ip_addr,
                to: dst_ip_addr,
            };

            // Process the packet
            if let Err(e) = self.conn.recv(&mut packet.data, recv_info) {
                error!("recv failed: {:?}, recv_info: {:?}", e, recv_info);
                continue;
            }
        }

        // Quiche checks if a provided certificate is valid and aborts if not. However, we need to check if the peer provided one at all.
        if self.conn.peer_cert().is_none() {
            debug!("no client certificate provided yet");
            if self.client_cert_timer.elapsed() > std::time::Duration::from_secs(5) {
                warn!("closing connection due to missing client certificate");
                self.conn.close(true, 0x100, b"no client certificate")?;
            }
            return Ok(());
        }

        let mut buf = [0; MAX_DATAGRAM_SIZE];
        if self.conn.is_established() {
            // Iterate over readable streams.
            for stream_id in self.conn.readable() {
                // Stream is readable, read until there's no more data.
                while let Ok((read, _)) = self.conn.stream_recv(stream_id, &mut buf) {
                    println!("Got {} bytes on stream {}", read, stream_id);

                    if stream_id == 0 {
                        // Handle data on stream 0 (TCP stream)
                        self.tcp_stream
                            .as_mut()
                            .unwrap()
                            .write_all(&buf[..read])
                            .await?;
                    } else {
                        warn!("Received data on unknown stream {}", stream_id);
                    }
                }
            }
        }

        Ok(())
    }
}
