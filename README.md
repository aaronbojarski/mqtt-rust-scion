# mqtt-rust-scion
mqtt-rust-scion provides a client and proxy implementation for MQTT over SCION.

The client is a regular MQTT client that can connect to MQTT brokers over the SCION network. The proxy acts as a bridge between the client and stardard MQTT brokers, allowing clients to communicate with brokers that do not natively support SCION.

For the MQTT protocol implementation, this project uses the [mqtt-protocol-core](https://crates.io/crates/mqtt-protocol-core) crate. For transport security we use QUIC via the [quiche](https://crates.io/crates/quiche) crate. SCION networking is provided by the [scion-sdk](https://crates.io/crates/scion-sdk) crate.

## Proxy Modes
The proxy can be started in two different modes:
- QuicEndpoint: In this mode, the the proxy is the QUIC endpoint that terminates the QUIC connection from the client. The proxy then establishes a separate (TCP) connection to the MQTT broker.
- UdpEndpoint: In this mode, the proxy forwards UDP packets between the client and the broker. The QUIC connection is established between the client and the broker through the proxy. The proxy translates between UDP/SCION packets and UDP/IP packets.

## TODO:
- Make client code a library that can be used by other applications