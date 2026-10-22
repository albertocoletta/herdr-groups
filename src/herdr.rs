use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

const CALL_TIMEOUT: Duration = Duration::from_secs(5);

pub trait Herdr {
    fn call(&self, method: &str, params: Value) -> Result<Value, Error>;
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Herdr { code: String, message: String },
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(error) => write!(f, "socket: {error}"),
            Error::Herdr { code, message } => write!(f, "herdr {code}: {message}"),
            Error::Protocol(problem) => write!(f, "protocol: {problem}"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::Io(error)
    }
}

pub struct Client {
    socket: PathBuf,
}

impl Client {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    pub fn from_env() -> Option<Self> {
        std::env::var_os("HERDR_SOCKET_PATH").map(|socket| Self::new(socket.into()))
    }
}

impl Herdr for Client {
    fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
        let mut stream = UnixStream::connect(&self.socket)?;
        stream.set_read_timeout(Some(CALL_TIMEOUT))?;
        stream.set_write_timeout(Some(CALL_TIMEOUT))?;

        let request = json!({ "id": format!("groups:{method}"), "method": method, "params": params });
        writeln!(stream, "{request}")?;

        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        if line.is_empty() {
            return Err(Error::Protocol("connection closed before a response".into()));
        }
        let mut response: Value =
            serde_json::from_str(&line).map_err(|error| Error::Protocol(error.to_string()))?;

        if let Some(error) = response.get("error") {
            let text = |key: &str| error.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
            return Err(Error::Herdr { code: text("code"), message: text("message") });
        }
        response
            .get_mut("result")
            .map(Value::take)
            .ok_or_else(|| Error::Protocol("response without result".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread::{self, JoinHandle};
    use tempfile::TempDir;

    fn serve_once(response: &'static str) -> (TempDir, PathBuf, JoinHandle<Value>) {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            writeln!(&stream, "{response}").unwrap();
            serde_json::from_str(&line).unwrap()
        });
        (dir, socket, server)
    }

    #[test]
    fn sends_method_and_params_as_one_json_line() {
        let (_dir, socket, server) = serve_once(r#"{"id":"x","result":{}}"#);
        Client::new(socket).call("agent.view.clear", json!({ "source": "plugin:test" })).unwrap();
        let request = server.join().unwrap();
        assert_eq!(request["method"], "agent.view.clear");
        assert_eq!(request["params"]["source"], "plugin:test");
    }

    #[test]
    fn returns_the_result() {
        let (_dir, socket, server) = serve_once(r#"{"id":"x","result":{"type":"agent_view","active":true}}"#);
        let result = Client::new(socket).call("agent.view.set", json!({})).unwrap();
        server.join().unwrap();
        assert_eq!(result["active"], true);
    }

    #[test]
    fn maps_herdr_errors() {
        let (_dir, socket, server) = serve_once(r#"{"id":"x","error":{"code":"plugin_disabled","message":"off"}}"#);
        let error = Client::new(socket).call("agent.view.set", json!({})).unwrap_err();
        server.join().unwrap();
        assert!(matches!(error, Error::Herdr { ref code, .. } if code == "plugin_disabled"));
    }

    #[test]
    fn missing_socket_is_an_io_error() {
        let dir = TempDir::new().unwrap();
        let error = Client::new(dir.path().join("absent.sock")).call("session.snapshot", json!({})).unwrap_err();
        assert!(matches!(error, Error::Io(_)));
    }
}
