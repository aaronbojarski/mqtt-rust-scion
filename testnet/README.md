# Test Network Setup

To start the SCION test network, run the `testnet.sh` script.
```bash
sudo bash ./testnet/testnet.sh up
```

We use the [pocketscion-configurator](https://github.com/aaronbojarski/pocketscion-configurator) to set up the pocketscion instance inside the `pocketscion_ns` namespace. First, fetch and build the configurator or use the prebuilt binary.
```bash
cd testnet
curl -LO https://github.com/aaronbojarski/pocketscion-configurator/releases/download/v0.1.1/pocketscion-configurator-x86-64-deb.tar.gz
tar -xvf pocketscion-configurator-x86-64-deb.tar.gz
sudo ip netns exec pocketscion_ns ./pocketscion-configurator --config ./pocketscion_config.json
cd ..
```

All necessary certificats can be generated with the `generate_certs.sh` script.
```bash
cd testnet
bash ./generate_certs.sh
cd ..
```

Download and start emqx broker inside the `server_ns` namespace.
```bash
cd ~
wget https://www.emqx.com/en/downloads/enterprise/6.1.0/emqx-enterprise-6.1.0-ubuntu24.04-amd64.tar.gz
mkdir -p emqx && tar -zxvf emqx-enterprise-6.1.0-ubuntu24.04-amd64.tar.gz -C emqx
sudo ip netns exec server_ns ~/emqx/bin/emqx foreground
```

To enable quic support in emqx, add the following lines to the `emqx/etc/emqx.conf` file before starting the broker. The key and cert files can be copied from the proxy certificates generated earlier.
```
listeners.quic.default {
  enabled = true
  bind = "0.0.0.0:14567"
  keyfile = "${EMQX_ETC_DIR}/certs/key.pem"
  certfile = "${EMQX_ETC_DIR}/certs/cert.pem"
}
```

Client and proxy binaries can be built with cargo.
```bash
cargo build --release
```

Then start the proxy in the corresponding namespace.
```bash
sudo ip netns exec server_ns ./target/release/mqtt-rust-scion proxy --listen [2-3,10.0.200.10]:4433 --ca-cert ./testnet/ca-cert.pem --cert ./testnet/proxy-cert.pem --key ./testnet/proxy-key.pem --endhost-api http://10.0.200.20:10231 --mode QuicEndpoint --mqtt-broker 127.0.0.1:1883
```

The proxy can instead also be started in UdpEndpoint mode. In this mode the QUIC connection is established between the client and the broker through the proxy. The proxy is just translating between UDP/SCION packets and UDP/IP packets.
```bash
sudo ip netns exec server_ns ./target/release/mqtt-rust-scion proxy --listen [2-3,10.0.200.10]:4433 --ca-cert ./testnet/ca-cert.pem --cert ./testnet/proxy-cert.pem --key ./testnet/proxy-key.pem --endhost-api http://10.0.200.20:10231 --mode UdpEndpoint --mqtt-broker 127.0.0.1:14567 --log debug
```

Then start the subscribing client in another terminal.
```bash
sudo ip netns exec client_ns ./target/release/mqtt-rust-scion client [2-3,10.0.200.10]:4433 --host localhost --ca-cert ./testnet/ca-cert.pem --cert ./testnet/client-cert.pem --key ./testnet/client-key.pem --endhost-api http://10.0.100.20:10143 --snap-token ./testnet/snap.token --client-id client1 subscribe test
```

Then start the publishing client in another terminal.
```bash
sudo ip netns exec client_ns ./target/release/mqtt-rust-scion client [2-3,10.0.200.10]:4433 --host localhost --ca-cert ./testnet/ca-cert.pem --cert ./testnet/client-cert.pem --key ./testnet/client-key.pem --endhost-api http://10.0.100.20:10143 --snap-token ./testnet/snap.token --client-id client2 publish test 12345
```

Finally, the test network can be torn down.
```bash
sudo bash ./testnet/testnet.sh down
```