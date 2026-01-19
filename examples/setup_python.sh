#!/bin/bash
# Quick setup script for Python bindings

set -e

echo "========================================="
echo "MQTT-SCION Python Bindings Setup"
echo "========================================="
echo ""

# Check if maturin is installed
if ! command -v maturin &> /dev/null; then
    echo "⚠️  Maturin is not installed."
    echo "Installing maturin..."
    pip install maturin
    echo "✓ Maturin installed"
else
    echo "✓ Maturin is already installed"
fi

echo ""
echo "Building Python module in development mode..."
echo "This will build the Rust code and install it in your Python environment."
echo ""

# Build and install in development mode
maturin develop --features python-bindings

echo ""
echo "========================================="
echo "✓ Installation complete!"
echo "========================================="
echo ""
echo "You can now use the module in Python:"
echo ""
echo "  from mqtt_rust_scion import PyMqttClient, PyClientConfig"
echo ""
echo "Run the example:"
echo "  python examples/python_example.py"
echo ""
echo "For more information, see PYTHON_BINDINGS.md"
