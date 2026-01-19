# Quick Start - Python Bindings

## Installation

```bash
# Run the setup script
./setup_python.sh

# Or manually:
pip install maturin
maturin develop --features python-bindings
```

## Basic Usage

```python
import asyncio
from mqtt_rust_scion import PyMqttClient, PyClientConfig

async def main():
    # Configure
    config = PyClientConfig(
        client_id="my-client",
        endhost_api_address="http://localhost:30255",
        ca_cert_path="./testnet/ca-cert.pem",
        cert_path="./testnet/client-cert.pem",
        key_path="./testnet/client-key.pem",
        host="localhost",
        snap_token_path="./snap_token.token"
    )
    
    # Connect
    client = PyMqttClient(config)
    await client.connect("[1-ff00:0:110,10.0.1.0]:8883")
    
    # Subscribe
    await client.subscribe("test/topic")
    
    # Publish
    await client.publish("test/topic", b"Hello!")
    
    # Receive
    msg = await client.receive()
    print(f"{msg.topic}: {msg.payload_str()}")

asyncio.run(main())
```

See [PYTHON_BINDINGS.md](PYTHON_BINDINGS.md) for complete documentation.
