# mqtt-rust-scion
mqtt-rust-scion provides a client and proxy implementation for MQTT over SCION.

The client is a regular MQTT client that can connect to MQTT brokers over the SCION network. The proxy acts as a bridge between the client and standard MQTT brokers, allowing clients to communicate with brokers that do not natively support SCION. Ideally, the proxy is deployed close to the MQTT broker, e.g., in the same data center or or even on the same host.

For the MQTT protocol implementation, this project uses the [mqtt-protocol-core](https://crates.io/crates/mqtt-protocol-core) crate. For transport security we use QUIC via the [quiche](https://crates.io/crates/quiche) crate. SCION networking is provided by the [scion-sdk](https://crates.io/crates/scion-sdk) crate.


## Client Implementation
The client provides basic MQTT functionality, including connecting to a broker, publishing messages, and subscribing to topics. It supports MQTT v5.0 and uses QUIC over SCION for transport. It is built as a library that can be integrated into other Rust applications. At the moment, it has simple, but limited API.

```rust
let client_config = mqtt_rust_scion::client::ClientConfig {
    ...
};
let mut client = mqtt_rust_scion::client::Client::new(client_config);
client.connect("PROXY_SCION_ADDRESS").await?;

client.publish("topic1", b"Hello, MQTT over SCION!".to_vec()).await?;

client.subscribe("topic2").await?;
let message = client.rcv().await?;
```

Additionally to the library, a simple command-line client application is provided. It can be used to test the client functionality and connect to MQTT brokers over SCION. Its usage can be seen in the testnet setup. The cli is implemented in `src/main.rs`. The configuration options can be viewed there.


## Proxy Implementation
The proxy is implemented as a standalone application. Its usage can be seen in the testnet setup.

### Modes
The proxy can be started in two different modes:
- QuicEndpoint: In this mode, the the proxy is the QUIC endpoint that terminates the QUIC connection from the client. The proxy then establishes a separate (TCP) connection to the MQTT broker.
- UdpEndpoint: In this mode, the proxy forwards UDP packets between the client and the broker. The QUIC connection is established between the client and the broker through the proxy. The proxy translates between UDP/SCION packets and UDP/IP packets.


## Test Network Setup
A test network setup is provided in the `testnet` directory. It uses network namespaces and pocketscion for a local SCION network. It includes a client, proxy, and MQTT broker.


## Python Bindings
Python bindings for the MQTT client are provided via PyO3. The bindings expose the core functionality of the Rust client library to Python applications. See the [PYTHON_BINDINGS.md](docs/PYTHON_BINDINGS.md) and [PYTHON_QUICKSTART.md](docs/PYTHON_QUICKSTART.md) for more information.