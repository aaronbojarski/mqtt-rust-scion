use std::{fs, sync::Arc};

use anyhow::{Context, Result, anyhow};
use mqtt_protocol_core::mqtt;
use mqtt_protocol_core::mqtt::prelude::*;
use ring::rand::{SecureRandom, SystemRandom};
use scion_proto::path::policy::acl::AclPolicy;
use scion_stack::scionstack::ScionStackBuilder;
use tokio::sync::Notify;
use tracing::{Instrument, debug, error, info, info_span, trace, warn};

use crate::net::quic::{DEFAULT_TIMEOUT, KEEPALIVE_INTERVAL, MAX_DATAGRAM_SIZE};

pub const UDP_PACKET_BUFFER_SIZE: usize = 65535;

#[derive(Debug, Clone)]
pub struct MqttMessage {
    pub topic: String,
    pub qos: mqtt::packet::Qos,
    pub payload: Vec<u8>,
}

enum Request {
    Subscribe { topic: String },
    Publish { message: MqttMessage },
}

#[derive(Clone)]
pub struct ClientConfig {
    pub bind: Option<scion_proto::address::SocketAddr>,
    pub host: Option<String>,
    pub endhost_api_address: url::Url,
    pub snap_token_path: Option<std::path::PathBuf>,
    pub acl_policy: Option<AclPolicy>,
    pub ca_cert_path: std::path::PathBuf,
    pub cert_path: std::path::PathBuf,
    pub key_path: std::path::PathBuf,
    pub client_id: String,
}

pub struct Client {
    config: ClientConfig,
    connect_notification: Arc<Notify>,
    connected: bool,
    request_stream: Option<tokio::sync::mpsc::Sender<(Request, Arc<Notify>)>>,
    message_stream: Option<tokio::sync::mpsc::Receiver<MqttMessage>>,
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        Client {
            config,
            connect_notification: Arc::new(Notify::new()),
            connected: false,
            request_stream: None,
            message_stream: None,
        }
    }

    pub async fn connect(&mut self, remote: scion_proto::address::SocketAddr) -> Result<()> {
        let (message_sender, message_receiver) = tokio::sync::mpsc::channel(100);
        self.message_stream = Some(message_receiver);

        let (request_sender, request_receiver) = tokio::sync::mpsc::channel(1);
        self.request_stream = Some(request_sender);

        let mut connection = Connection::new(
            self.config.clone(),
            remote,
            self.connect_notification.clone(),
            message_sender,
            request_receiver,
        )
        .instrument(info_span!("MQTT Connection Initialization"))
        .await?;

        tokio::spawn(
            async move {
                if let Err(e) = connection.connection_handling().await {
                    error!("Connection handling error: {:?}", e);
                }
            }
            .instrument(info_span!("MQTT Connection Handler")),
        );

        self.connect_notification.notified().await;
        self.connected = true;
        info!("Client connected to remote {}", remote);
        Ok(())
    }

    pub async fn subscribe(&mut self, topic: &str) -> Result<()> {
        if !self.connected {
            anyhow::bail!("client is not connected");
        }

        let request = Request::Subscribe {
            topic: topic.to_string(),
        };
        let notify = Arc::new(Notify::new());
        if let Some(ref mut sender) = self.request_stream {
            sender
                .send((request, notify.clone()))
                .await
                .map_err(|e| anyhow!("failed to send subscribe request: {:?}", e))?;
        } else {
            anyhow::bail!("request stream is not available");
        }
        notify.notified().await;

        Ok(())
    }

    pub async fn publish(&mut self, topic: &str, payload: Vec<u8>) -> Result<()> {
        if !self.connected {
            anyhow::bail!("client is not connected");
        }

        let message = MqttMessage {
            topic: topic.to_string(),
            qos: mqtt::packet::Qos::AtLeastOnce,
            payload,
        };
        let request = Request::Publish { message };
        let notify = Arc::new(Notify::new());
        if let Some(ref mut sender) = self.request_stream {
            sender
                .send((request, notify.clone()))
                .await
                .map_err(|e| anyhow!("failed to send publish request: {:?}", e))?;
        } else {
            anyhow::bail!("request stream is not available");
        }
        notify.notified().await;

        Ok(())
    }

    pub async fn rcv(&mut self) -> Result<MqttMessage> {
        // Implementation of the rcv method
        if let Some(ref mut stream) = self.message_stream {
            if let Some(packet) = stream.recv().await {
                return Ok(packet);
            }
        }
        Err(anyhow!("No message stream available"))
    }
}

struct Connection {
    socket: scion_stack::scionstack::UdpScionSocket,
    remote: scion_proto::address::SocketAddr,
    conn: quiche::Connection,
    mqtt_conn: mqtt::GenericConnection<mqtt::role::Client, u16>,
    client_id: String,
    mqtt_init_sent: bool,
    connect_notify: Arc<Notify>,
    message_sender: tokio::sync::mpsc::Sender<MqttMessage>,
    request_receiver: tokio::sync::mpsc::Receiver<(Request, Arc<Notify>)>,
    current_request: Option<(Request, u16, Arc<Notify>)>,
}

impl Connection {
    async fn new(
        config: ClientConfig,
        remote: scion_proto::address::SocketAddr,
        connect_notify: Arc<Notify>,
        message_sender: tokio::sync::mpsc::Sender<MqttMessage>,
        request_receiver: tokio::sync::mpsc::Receiver<(Request, Arc<Notify>)>,
    ) -> Result<Self> {
        let mut quic_config = crate::net::quic::configure_quic(
            &config.ca_cert_path,
            &config.cert_path,
            &config.key_path,
        )?;

        let mut builder = ScionStackBuilder::new(config.endhost_api_address.clone());
        if let Some(token_path) = &config.snap_token_path {
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
        if let Some(bind) = &config.bind
            && !local_ias.contains(&bind.isd_asn())
        {
            anyhow::bail!(
                "configured bind ISD-AS {} is not among the local ASes of the SCION stack: {:?}",
                bind.isd_asn(),
                local_ias
            );
        }

        let mut socket_config = scion_stack::scionstack::SocketConfig::new();
        if let Some(policy) = &config.acl_policy {
            debug!("Using ACL policy: {:?}", policy);
            socket_config = socket_config.with_path_policy(policy.clone());
        } else {
            debug!("No ACL policy specified, using default (allow all)");
        }

        let socket = client_network_stack
            .bind_with_config(config.bind, socket_config)
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
        let quic_conn = quiche::connect(
            config.host.as_deref(),
            &scid,
            local_addr.local_address().ok_or(anyhow::anyhow!(
                "failed to get local address from {:?}",
                local_addr
            ))?,
            remote.local_address().ok_or(anyhow::anyhow!(
                "failed to get remote address from {:?}",
                remote
            ))?,
            &mut quic_config,
        )?;

        let mut mqtt_conn = mqtt::Connection::<mqtt::role::Client>::new(mqtt::Version::V5_0);
        mqtt_conn.set_auto_pub_response(true);

        Ok(Connection {
            socket,
            remote,
            conn: quic_conn,
            mqtt_conn,
            client_id: config.client_id,
            mqtt_init_sent: false,
            connect_notify,
            message_sender,
            request_receiver,
            current_request: None,
        })
    }

    async fn connection_handling(&mut self) -> Result<()> {
        let mut buf = [0; UDP_PACKET_BUFFER_SIZE];
        let mut send_buf = [0; MAX_DATAGRAM_SIZE];

        let local_addr = self.socket.local_addr();

        // Send initial packet
        let (write, send_info) = self.conn.send(&mut send_buf)?;
        if let Err(e) = self
            .socket
            .send_to(
                &send_buf[..write],
                scion_proto::address::SocketAddr::from_std(self.remote.isd_asn(), send_info.to),
            )
            .await
        {
            error!("initial send_to failed to write {} bytes: {:?}", write, e);
            return Err(anyhow!("initial send failed: {:?}", e));
        }

        let mut keepalive_interval =
            tokio::time::interval(std::time::Duration::from_millis(KEEPALIVE_INTERVAL));

        // Main loop
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

                // Receive datagram from UDP socket and pass to QUIC
                Ok((len, src)) = self.socket.recv_from(&mut buf) => {
                    debug!("received {} bytes on socket from {}", len, src);
                    self.process_udp_packet(&mut buf[..len], src, local_addr).await?;
                }

                // Process requests from client
                Some((request, notify)) = self.request_receiver.recv() => {
                    match request {
                        Request::Subscribe { topic } => {
                            // Build SUBSCRIBE packet
                            let sub_opts = mqtt::packet::SubOpts::new().set_qos(mqtt::packet::Qos::AtLeastOnce);
                            let sub_entry = mqtt::packet::SubEntry::new(topic.clone(), sub_opts).unwrap();
                            let subscribe_packet = mqtt::packet::v5_0::Subscribe::builder()
                                .packet_id(self.mqtt_conn.acquire_packet_id()
                                    .map_err(|_| anyhow!("no packet ID available for SUBSCRIBE packet"))?)
                                .entries(vec![sub_entry])
                                .build()
                                .map_err(|e| anyhow!("failed to build SUBSCRIBE packet: {}", e))?;

                            let events = self.mqtt_conn.checked_send(subscribe_packet);
                            self.handle_events(events)
                                .map_err(|e| anyhow!("failed to handle events: {}", e))?;
                            debug!("sent SUBSCRIBE packet for topic '{}' on stream {}", topic, 0);
                        }
                        Request::Publish { message } => {
                            // Build PUBLISH packet
                            let mut publish_builder = mqtt::packet::v5_0::Publish::builder()
                                .topic_name(&message.topic)
                                .unwrap()
                                .payload(message.payload)
                                .qos(message.qos);
                            let packet_id = self.mqtt_conn.acquire_packet_id()
                                .map_err(|_| anyhow!("no packet ID available for PUBLISH packet"))?;
                            publish_builder = publish_builder.packet_id(packet_id);
                            let publish_packet = publish_builder.build()
                                .map_err(|e| anyhow!("failed to build PUBLISH packet: {}", e))?;

                            let events = self.mqtt_conn.checked_send(publish_packet);
                            self.handle_events(events)
                                .map_err(|e| anyhow!("failed to handle events: {}", e))?;
                            debug!("sent PUBLISH packet for topic '{}' on stream {}", message.topic, 0);
                        }
                    }
                    notify.notify_one();
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
                let (write, send_info) = match self.conn.send(&mut send_buf) {
                    Ok(v) => v,
                    Err(quiche::Error::Done) => break,
                    Err(e) => {
                        error!("send failed: {:?}", e);
                        break;
                    }
                };

                if let Err(e) = self
                    .socket
                    .send_to(
                        &send_buf[..write],
                        scion_proto::address::SocketAddr::from_std(
                            self.remote.isd_asn(),
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

        self.conn.recv(data, recv_info)?;

        if self.conn.is_established() && !self.mqtt_init_sent {
            // Send MQTT CONNECT packet
            // Build CONNECT packet
            let connect_packet = mqtt::packet::v5_0::Connect::builder()
                .client_id(&self.client_id)
                .unwrap()
                .build()
                .map_err(|e| anyhow!("failed to build CONNECT packet: {}", e))?;

            // Send through connection (returns events to handle)
            let events = self.mqtt_conn.checked_send(connect_packet);
            self.handle_events(events)
                .map_err(|e| anyhow!("failed to handle events: {}", e))?;

            self.mqtt_init_sent = true;
            debug!("sent MQTT CONNECT packet on stream {}", 0);
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
                    debug!("Sent packet: {packet_type}");
                }
                mqtt::connection::Event::NotifyPacketReceived(packet) => match packet {
                    mqtt::packet::Packet::V5_0Connack(connack) => {
                        let reason_code = connack.reason_code();
                        debug!("CONNACK received: {reason_code:?}");
                        self.connect_notify.notify_one();
                    }
                    mqtt::packet::Packet::V5_0Puback(puback) => {
                        let packet_id = puback.packet_id();
                        debug!("PUBACK received for packet ID: {packet_id}");
                        if let Some((Request::Publish { message }, id, notify)) =
                            &self.current_request
                            && *id == packet_id
                            && matches!(message.qos, mqtt::packet::Qos::AtLeastOnce)
                        {
                            notify.notify_one();
                            self.current_request = None;
                        }
                    }
                    mqtt::packet::Packet::V5_0Pubrec(pubrec) => {
                        let packet_id = pubrec.packet_id();
                        debug!("PUBREC received for packet ID: {packet_id}");
                        if let Some((Request::Publish { message }, id, notify)) =
                            &self.current_request
                            && *id == packet_id
                            && matches!(message.qos, mqtt::packet::Qos::ExactlyOnce)
                        {
                            notify.notify_one();
                            self.current_request = None;
                        }
                    }
                    mqtt::packet::Packet::V5_0Pubcomp(pubcomp) => {
                        let packet_id = pubcomp.packet_id();
                        debug!("PUBCOMP received for packet ID: {packet_id}");
                    }
                    mqtt::packet::Packet::V5_0Publish(publish) => {
                        let topic = publish.topic_name();
                        let payload = publish.payload().as_slice().to_vec();
                        let message = MqttMessage {
                            topic: topic.to_string(),
                            qos: publish.qos(),
                            payload,
                        };
                        if let Err(e) = self.message_sender.try_send(message) {
                            error!("Failed to send received message to channel: {:?}", e);
                        } else {
                            debug!("PUBLISH received for topic: {topic}");
                        }
                    }
                    mqtt::packet::Packet::V5_0Subscribe(subscribe) => {
                        let packet_id = subscribe.packet_id();
                        warn!("SUBSCRIBE received for packet ID: {packet_id}");
                    }
                    mqtt::packet::Packet::V5_0Suback(suback) => {
                        let packet_id = suback.packet_id();
                        debug!("SUBACK received for packet ID: {packet_id}");
                        if let Some((Request::Subscribe { .. }, id, notify)) = &self.current_request
                            && *id == packet_id
                        {
                            notify.notify_one();
                            self.current_request = None;
                        }
                    }
                    _ => {
                        let packet_type = packet.packet_type();
                        warn!("Received packet: {packet_type}");
                    }
                },
                mqtt::connection::Event::NotifyPacketIdReleased(packet_id) => {
                    debug!("Packet ID {packet_id} released");
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
