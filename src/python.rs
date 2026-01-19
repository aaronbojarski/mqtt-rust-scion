use pyo3::exceptions::{PyConnectionError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use std::path::PathBuf;
use std::sync::Arc;

/// Python wrapper for MqttMessage
#[pyclass]
#[derive(Clone)]
pub struct PyMqttMessage {
    #[pyo3(get)]
    pub topic: String,
    #[pyo3(get)]
    pub payload: Vec<u8>,
}

#[pymethods]
impl PyMqttMessage {
    #[new]
    fn new(topic: String, payload: Vec<u8>) -> Self {
        PyMqttMessage { topic, payload }
    }

    fn __repr__(&self) -> String {
        format!(
            "PyMqttMessage(topic='{}', payload_len={})",
            self.topic,
            self.payload.len()
        )
    }

    /// Get payload as string (UTF-8)
    fn payload_str(&self) -> PyResult<String> {
        String::from_utf8(self.payload.clone())
            .map_err(|e| PyValueError::new_err(format!("Invalid UTF-8: {}", e)))
    }
}

impl From<crate::client::MqttMessage> for PyMqttMessage {
    fn from(msg: crate::client::MqttMessage) -> Self {
        PyMqttMessage {
            topic: msg.topic,
            payload: msg.payload,
        }
    }
}

/// Python wrapper for ClientConfig
#[pyclass]
#[derive(Clone)]
pub struct PyClientConfig {
    #[pyo3(get, set)]
    pub client_id: String,
    #[pyo3(get, set)]
    pub endhost_api_address: String,
    #[pyo3(get, set)]
    pub ca_cert_path: String,
    #[pyo3(get, set)]
    pub cert_path: String,
    #[pyo3(get, set)]
    pub key_path: String,
    #[pyo3(get, set)]
    pub bind_address: Option<String>,
    #[pyo3(get, set)]
    pub host: Option<String>,
    #[pyo3(get, set)]
    pub snap_token_path: Option<String>,
}

#[pymethods]
impl PyClientConfig {
    #[new]
    #[pyo3(signature = (client_id, endhost_api_address, ca_cert_path, cert_path, key_path, bind_address=None, host=None, snap_token_path=None))]
    fn new(
        client_id: String,
        endhost_api_address: String,
        ca_cert_path: String,
        cert_path: String,
        key_path: String,
        bind_address: Option<String>,
        host: Option<String>,
        snap_token_path: Option<String>,
    ) -> Self {
        PyClientConfig {
            client_id,
            endhost_api_address,
            ca_cert_path,
            cert_path,
            key_path,
            bind_address,
            host,
            snap_token_path,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "PyClientConfig(client_id='{}', endhost_api_address='{}')",
            self.client_id, self.endhost_api_address
        )
    }
}

impl TryFrom<PyClientConfig> for crate::client::ClientConfig {
    type Error = String;

    fn try_from(config: PyClientConfig) -> Result<Self, Self::Error> {
        let endhost_api_address = url::Url::parse(&config.endhost_api_address)
            .map_err(|e| format!("Invalid endhost API URL: {}", e))?;

        let bind = if let Some(bind_str) = &config.bind_address {
            Some(
                bind_str
                    .parse::<scion_proto::address::SocketAddr>()
                    .map_err(|e| format!("Invalid bind address: {}", e))?,
            )
        } else {
            None
        };

        let snap_token_path = config.snap_token_path.map(PathBuf::from);

        Ok(crate::client::ClientConfig {
            bind,
            host: config.host,
            endhost_api_address,
            snap_token_path,
            acl_policy: None, // TODO: expose ACL policy to Python if needed
            ca_cert_path: PathBuf::from(config.ca_cert_path),
            cert_path: PathBuf::from(config.cert_path),
            key_path: PathBuf::from(config.key_path),
            client_id: config.client_id,
        })
    }
}

/// Python wrapper for the MQTT SCION Client
#[pyclass]
pub struct PyMqttClient {
    inner: Arc<tokio::sync::Mutex<crate::client::Client>>,
}

#[pymethods]
impl PyMqttClient {
    #[new]
    fn new(config: PyClientConfig) -> PyResult<Self> {
        let rust_config =
            crate::client::ClientConfig::try_from(config).map_err(|e| PyValueError::new_err(e))?;

        let client = crate::client::Client::new(rust_config);

        Ok(PyMqttClient {
            inner: Arc::new(tokio::sync::Mutex::new(client)),
        })
    }

    /// Connect to the MQTT broker
    ///
    /// Args:
    ///     remote (str): Remote address in format "[ISD-AS,IP]:port"
    ///                   e.g., "[1-ff00:0:110,10.0.0.1]:4433"
    fn connect<'p>(&self, py: Python<'p>, remote: String) -> PyResult<&'p PyAny> {
        let inner = self.inner.clone();

        pyo3_asyncio::tokio::future_into_py(py, async move {
            let remote_addr = parse_scion_address(&remote).map_err(|e| PyValueError::new_err(e))?;

            let mut client = inner.lock().await;
            client
                .connect(remote_addr)
                .await
                .map_err(|e| PyConnectionError::new_err(format!("Connection failed: {}", e)))?;

            Ok(())
        })
    }

    /// Subscribe to a topic
    ///
    /// Args:
    ///     topic (str): MQTT topic to subscribe to
    fn subscribe<'p>(&self, py: Python<'p>, topic: String) -> PyResult<&'p PyAny> {
        let inner = self.inner.clone();

        pyo3_asyncio::tokio::future_into_py(py, async move {
            let mut client = inner.lock().await;
            client
                .subscribe(&topic)
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Subscribe failed: {}", e)))?;

            Ok(())
        })
    }

    /// Publish a message to a topic
    ///
    /// Args:
    ///     topic (str): MQTT topic to publish to
    ///     payload (bytes): Message payload
    fn publish<'p>(&self, py: Python<'p>, topic: String, payload: Vec<u8>) -> PyResult<&'p PyAny> {
        let inner = self.inner.clone();

        pyo3_asyncio::tokio::future_into_py(py, async move {
            let mut client = inner.lock().await;
            client
                .publish(&topic, payload)
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Publish failed: {}", e)))?;

            Ok(())
        })
    }

    /// Receive a message from subscribed topics
    ///
    /// Returns:
    ///     PyMqttMessage: The received message
    fn receive<'p>(&self, py: Python<'p>) -> PyResult<&'p PyAny> {
        let inner = self.inner.clone();

        pyo3_asyncio::tokio::future_into_py(py, async move {
            let mut client = inner.lock().await;
            let msg = client
                .rcv()
                .await
                .map_err(|e| PyRuntimeError::new_err(format!("Receive failed: {}", e)))?;

            Ok(PyMqttMessage::from(msg))
        })
    }

    fn __repr__(&self) -> String {
        "PyMqttClient()".to_string()
    }
}

/// Parse a SCION address string in format "[ISD-AS,IP]:port"
/// Example: "[1-ff00:0:110,10.0.0.1]:8883"
fn parse_scion_address(addr: &str) -> Result<scion_proto::address::SocketAddr, String> {
    addr.parse::<scion_proto::address::SocketAddr>()
        .map_err(|e| format!("Invalid SCION address '{}': {}", addr, e))
}

/// Initialize the Python module
#[pymodule]
fn mqtt_rust_scion(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<PyMqttClient>()?;
    m.add_class::<PyClientConfig>()?;
    m.add_class::<PyMqttMessage>()?;
    Ok(())
}
