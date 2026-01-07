# mqtt-rust-scion
mqtt-rust-scion provides a client and proxy implementation for MQTT over SCION.

The client is a regular MQTT client that can connect to MQTT brokers over the SCION network. The proxy acts as a bridge between the client and stardard MQTT brokers, allowing clients to communicate with brokers that do not natively support SCION.

For the MQTT protocol implementation, this project uses the [mqtt-protocol-core](https://crates.io/crates/mqtt-protocol-core) crate. For transport security we use QUIC via the [quiche](https://crates.io/crates/quiche) crate. SCION networking is provided by the [scion-sdk](https://crates.io/crates/scion-sdk) crate.

