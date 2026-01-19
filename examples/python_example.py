#!/usr/bin/env python3
"""
Example Python script demonstrating MQTT over SCION using the Python bindings.

This script shows how to:
1. Create a client with configuration
2. Connect to an MQTT broker over SCION
3. Subscribe to topics
4. Publish messages
5. Receive messages
"""

import asyncio
from mqtt_rust_scion import PyMqttClient, PyClientConfig


async def pub_sub_example():
    """
    Example showing concurrent publishing and subscribing
    """
    # Create subscriber client
    subscriber_config = PyClientConfig(
        client_id="python-mqtt-subscriber",
        endhost_api_address="http://10.0.100.20:10143",
        ca_cert_path="./testnet/ca-cert.pem",
        cert_path="./testnet/client-cert.pem",
        key_path="./testnet/client-key.pem",
        host="localhost",
        snap_token_path="./testnet/snap.token",
    )

    # Create publisher client
    publisher_config = PyClientConfig(
        client_id="python-mqtt-publisher",
        endhost_api_address="http://10.0.100.20:10143",
        ca_cert_path="./testnet/ca-cert.pem",
        cert_path="./testnet/client-cert.pem",
        key_path="./testnet/client-key.pem",
        host="localhost",
        snap_token_path="./testnet/snap.token",
    )

    subscriber_client = PyMqttClient(subscriber_config)
    publisher_client = PyMqttClient(publisher_config)

    print("Created MQTT clients with SCION configuration")

    remote_address = "[2-3,10.0.200.10]:4433"

    # Connect both clients
    await subscriber_client.connect(remote_address)
    await publisher_client.connect(remote_address)

    print("Connected to MQTT broker over SCION")

    await subscriber_client.subscribe("sensors/#")  # Subscribe to all sensor topics
    print("Subscribed to topic: sensors/#")

    async def publisher():
        """Publish sensor readings periodically"""
        print("Starting publisher...")
        for i in range(10):
            await asyncio.sleep(2)
            temp = 20 + i * 0.5
            payload = f"Temperature: {temp}°C".encode()
            await publisher_client.publish("sensors/temperature", payload)
            print(f"Published: {payload}")

    async def subscriber():
        """Receive and print messages"""
        print("Starting subscriber...")
        for _ in range(10):
            msg = await subscriber_client.receive()
            print(f"Received on {msg.topic}: {msg.payload_str()}")

    # Run publisher and subscriber concurrently
    await asyncio.gather(publisher(), subscriber())


if __name__ == "__main__":
    asyncio.run(pub_sub_example())
