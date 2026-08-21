//! Herdr socket API client.
//!
//! The server speaks newline-delimited JSON over a Unix socket: one request
//! object per line, one response object per line. Talking to it directly
//! avoids a process spawn per call — a Swap or a Layout rebuild issues a dozen
//! requests — and reaches methods the `herdr` CLI does not expose, such as
//! `layout.export` and `layout.set_split_ratio`.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

/// A structured Herdr API error (`{"error":{"code","message"}}`).
#[derive(Debug, Clone)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    /// Whether the error means "the thing you named is gone", which callers
    /// turn into a refresh rather than a hard failure (spec §15.1).
    pub fn is_not_found(&self) -> bool {
        self.code.ends_with("_not_found")
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for ApiError {}

/// Client for the Herdr server.
///
/// The server serves exactly one request per connection and then closes it, so
/// every call dials the socket afresh. That is still far cheaper than spawning
/// a `herdr` CLI process per call, which is what the alternative would be.
pub struct Herdr {
    path: PathBuf,
    /// `RefCell` so the whole API can take `&self`; a plugin process is
    /// single-threaded and issues one request at a time.
    next_id: RefCell<u64>,
}

/// Socket path, from the environment Herdr gives every pane and plugin process.
pub fn socket_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HERDR_SOCKET_PATH") {
        if !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".config/herdr/herdr.sock"))
}

impl Herdr {
    /// Prepare a client, failing early if the server is not reachable.
    pub fn connect() -> Result<Self> {
        Self::at(socket_path()?)
    }

    /// Prepare a client for a socket other than this process's own.
    ///
    /// Each Herdr session has its own server and its own socket, and no
    /// session's API can see any other. Reading a second session — what the
    /// Sessions plugin does to summarise the ones you are not in — means
    /// dialling that session's socket directly.
    pub fn at(path: impl Into<PathBuf>) -> Result<Self> {
        let client = Self {
            path: path.into(),
            next_id: RefCell::new(0),
        };
        client.dial()?;
        Ok(client)
    }

    fn dial(&self) -> Result<UnixStream> {
        let stream = UnixStream::connect(&self.path).with_context(|| {
            format!(
                "could not reach the Herdr server at {} — is Herdr running?",
                self.path.display()
            )
        })?;
        // Long enough for a slow server, short enough that a wedged socket
        // does not hang a popup forever.
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        stream.set_write_timeout(Some(Duration::from_secs(15)))?;
        Ok(stream)
    }

    /// Issue one request and return its `result` object.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = {
            let mut next = self.next_id.borrow_mut();
            *next += 1;
            format!("plugin-kit:{}", *next)
        };

        let request = json!({ "id": id, "method": method, "params": params });
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');

        let mut stream = self.dial()?;
        stream
            .write_all(line.as_bytes())
            .with_context(|| format!("could not send {method} to Herdr"))?;
        stream.flush()?;

        let mut response = String::new();
        let read = BufReader::new(&stream)
            .read_line(&mut response)
            .with_context(|| format!("could not read the response to {method}"))?;
        if read == 0 {
            return Err(anyhow!("the Herdr server closed the connection"));
        }

        let value: Value = serde_json::from_str(response.trim())
            .with_context(|| format!("{method} returned malformed JSON"))?;

        if let Some(err) = value.get("error") {
            return Err(ApiError {
                code: err
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                message: err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the request failed")
                    .to_string(),
            }
            .into());
        }

        value
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("{method} returned no result"))
    }

    /// Issue a request and decode one named field of the result.
    pub fn call_field<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
        field: &str,
    ) -> Result<T> {
        let result = self.call(method, params)?;
        let raw = result
            .get(field)
            .cloned()
            .ok_or_else(|| anyhow!("{method} returned no `{field}`"))?;
        serde_json::from_value(raw).with_context(|| format!("could not decode `{field}`"))
    }
}

/// Downcast an error to a Herdr API error, if it is one.
pub fn api_error(err: &anyhow::Error) -> Option<&ApiError> {
    err.downcast_ref::<ApiError>()
}
