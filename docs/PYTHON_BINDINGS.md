# Python Bindings for MQTT-SCION

This document explains how to build and use the Python bindings for the MQTT over SCION client.

## Prerequisites

1. **Rust toolchain** (for building)
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **Python 3.8+** with development headers
   ```bash
   # Ubuntu/Debian
   sudo apt install python3-dev python3-pip
   ```

3. **Maturin** (Python/Rust build tool)
   ```bash
   pip install maturin
   ```

## Building the Python Module

### Option 1: Development Build (Recommended for Testing)

Build and install in development mode (editable install):

```bash
# Build with the python-bindings feature
maturin develop --features python-bindings

# Or for release mode (optimized):
maturin develop --release --features python-bindings
```

This installs the module directly into your current Python environment.

### Option 2: Build Wheel Package

Build a distributable wheel file:

```bash
# Build wheel
maturin build --release --features python-bindings

# Install the wheel
pip install target/wheels/mqtt_rust_scion-*.whl
```

### Option 3: Using pip (Direct Install)

```bash
pip install . --config-settings="--features=python-bindings"
```

## Usage

### Basic Example

```python
import asyncio
from mqtt_rust_scion import PyMqttClient, PyClientConfig

async def main():
    # Configure the client
    config = PyClientConfig(
        client_id="my-python-client",
        endhost_api_address="http://localhost:30255",
        ca_cert_path="./testnet/ca-cert.pem",
        cert_path="./testnet/client-cert.pem",
        key_path="./testnet/client-key.pem",
        host="localhost",
        snap_token_path="./snap_token.token",
    )

    # Create and connect
    client = PyMqttClient(config)
    await client.connect("[1-ff00:0:110,10.0.1.0]:8883")

    # Subscribe to a topic
    await client.subscribe("test/topic")

    # Publish a message
    await client.publish("test/topic", b"Hello SCION!")

    # Receive a message
    msg = await client.receive()
    print(f"Received on {msg.topic}: {msg.payload_str()}")

asyncio.run(main())
```

### Running the Example

A complete example is provided in `examples/python_example.py`:

```bash
python examples/python_example.py
```

Make sure to:
1. Have the SCION network running (testnet)
2. Update the configuration paths in the example
3. Update the remote address to match your broker

## API Reference

### PyClientConfig

Configuration for the MQTT client.

**Constructor:**
```python
PyClientConfig(
    client_id: str,
    endhost_api_address: str,
    ca_cert_path: str,
    cert_path: str,
    key_path: str,
    bind_address: Optional[str] = None,
    host: Optional[str] = None,
    snap_token_path: Optional[str] = None
)
```

**Parameters:**
- `client_id`: Unique identifier for this MQTT client
- `endhost_api_address`: URL of the SCION daemon API (e.g., `http://localhost:30255`)
- `ca_cert_path`: Path to CA certificate file
- `cert_path`: Path to client certificate file
- `key_path`: Path to client private key file
- `bind_address`: Optional local SCION address to bind to (format: `[ISD-AS,IP]:port`)
- `host`: Optional hostname for TLS verification
- `snap_token_path`: Optional path to SNAP authentication token file

### PyMqttClient

The main MQTT client class.

**Constructor:**
```python
client = PyMqttClient(config: PyClientConfig)
```

**Methods:**

#### `async connect(remote: str) -> None`
Connect to an MQTT broker.

```python
await client.connect("[1-ff00:0:110,10.0.1.0]:8883")
```

**Parameters:**
- `remote`: SCION address of the broker in format `[ISD-AS,IP]:port`

**Raises:**
- `ConnectionError`: If connection fails

#### `async subscribe(topic: str) -> None`
Subscribe to an MQTT topic.

```python
await client.subscribe("sensors/temperature")
await client.subscribe("alerts/#")  # Wildcards supported
```

**Parameters:**
- `topic`: MQTT topic pattern to subscribe to

**Raises:**
- `RuntimeError`: If not connected or subscription fails

#### `async publish(topic: str, payload: bytes) -> None`
Publish a message to a topic.

```python
await client.publish("sensors/temp", b"25.5")
await client.publish("status", "online".encode())
```

**Parameters:**
- `topic`: MQTT topic to publish to
- `payload`: Message payload as bytes

**Raises:**
- `RuntimeError`: If not connected or publish fails

#### `async receive() -> PyMqttMessage`
Receive a message from subscribed topics (blocking).

```python
msg = await client.receive()
print(msg.topic, msg.payload)
```

**Returns:**
- `PyMqttMessage`: The received message

**Raises:**
- `RuntimeError`: If receive fails or no stream available

### PyMqttMessage

Represents an MQTT message.

**Attributes:**
- `topic` (str): The message topic
- `payload` (bytes): The message payload

**Methods:**

#### `payload_str() -> str`
Get payload as UTF-8 string.

```python
msg = await client.receive()
text = msg.payload_str()  # Raises ValueError if not valid UTF-8
```

**Returns:**
- `str`: Payload decoded as UTF-8

**Raises:**
- `ValueError`: If payload is not valid UTF-8

## Advanced Usage

### Concurrent Publishing and Subscribing
This is not supported since the client serializes all operations with a mutex. 
You can use separate client instances for concurrent operations.


### Error Handling

```python
import asyncio
from mqtt_rust_scion import PyMqttClient, PyClientConfig

async def main():
    try:
        config = PyClientConfig(...)
        client = PyMqttClient(config)
        await client.connect("[1-ff00:0:110,10.0.1.0]:8883")
        await client.subscribe("test/topic")
        
        while True:
            msg = await client.receive()
            print(f"Received: {msg.payload_str()}")
            
    except ConnectionError as e:
        print(f"Connection failed: {e}")
    except RuntimeError as e:
        print(f"Runtime error: {e}")
    except ValueError as e:
        print(f"Value error: {e}")
    except KeyboardInterrupt:
        print("Interrupted by user")

asyncio.run(main())
```

### Multiple Clients

```python
async def main():
    # Client 1 - Publisher
    config1 = PyClientConfig(
        client_id="publisher",
        ...
    )
    publisher = PyMqttClient(config1)
    await publisher.connect("[1-ff00:0:110,10.0.1.0]:8883")
    
    # Client 2 - Subscriber
    config2 = PyClientConfig(
        client_id="subscriber",
        ...
    )
    subscriber = PyMqttClient(config2)
    await subscriber.connect("[1-ff00:0:110,10.0.1.0]:8883")
    await subscriber.subscribe("notifications/#")
    
    # Publish from client 1
    await publisher.publish("notifications/alert", b"Important message")
    
    # Receive on client 2
    msg = await subscriber.receive()
    print(f"Subscriber got: {msg.payload_str()}")
```

## Troubleshooting

### Build Errors

**"cannot find -lpython3.x"**
- Install Python development headers: `sudo apt install python3-dev`

**"feature `python-bindings` is required"**
- Make sure to build with: `maturin develop --features python-bindings`

**Rust compiler errors**
- Ensure you have the latest stable Rust: `rustup update stable`

### Runtime Errors

**ModuleNotFoundError: No module named 'mqtt_rust_scion'**
- Run `maturin develop --features python-bindings` to install the module

**ConnectionError**
- Check that the SCION daemon is running
- Verify the remote address format: `[ISD-AS,IP]:port`
- Ensure certificates are valid and paths are correct

**"client is not connected"**
- Call `await client.connect(...)` before `subscribe()`, `publish()`, or `receive()`

## Type Hints

For better IDE support, you can create a stub file `mqtt_rust_scion.pyi`:

```python
from typing import Optional

class PyClientConfig:
    client_id: str
    endhost_api_address: str
    ca_cert_path: str
    cert_path: str
    key_path: str
    bind_address: Optional[str]
    host: Optional[str]
    snap_token_path: Optional[str]
    
    def __init__(
        self,
        client_id: str,
        endhost_api_address: str,
        ca_cert_path: str,
        cert_path: str,
        key_path: str,
        bind_address: Optional[str] = None,
        host: Optional[str] = None,
        snap_token_path: Optional[str] = None
    ) -> None: ...

class PyMqttMessage:
    topic: str
    payload: bytes
    
    def __init__(self, topic: str, payload: bytes) -> None: ...
    def payload_str(self) -> str: ...

class PyMqttClient:
    def __init__(self, config: PyClientConfig) -> None: ...
    async def connect(self, remote: str) -> None: ...
    async def subscribe(self, topic: str) -> None: ...
    async def publish(self, topic: str, payload: bytes) -> None: ...
    async def receive(self) -> PyMqttMessage: ...
```
