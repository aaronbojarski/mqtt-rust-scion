pub mod quic;

/// Represents a UDP packet with SCION addressing.
///
/// Contains source and destination SCION addresses along with the packet payload.
pub struct UdpPacket {
    pub src: scion_proto::address::SocketAddr,
    pub dst: scion_proto::address::SocketAddr,
    pub data: Vec<u8>,
}
