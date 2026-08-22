//! Framing for the parent <-> per-user helper channel.
//!
//! Every message is a single line of JSON, optionally followed by `len` bytes
//! of raw payload. Keeping the payload out of the JSON avoids base64 inflation
//! on file reads and writes.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub op: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub to: String,
    /// Bytes of payload following this line.
    #[serde(default)]
    pub len: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(default)]
    pub len: usize,
}

impl Response {
    pub fn err(msg: impl std::fmt::Display) -> Self {
        Response { ok: false, error: Some(msg.to_string()), data: None, len: 0 }
    }
    pub fn ok_data(v: serde_json::Value) -> Self {
        Response { ok: true, error: None, data: Some(v), len: 0 }
    }
    pub fn ok_bytes(len: usize) -> Self {
        Response { ok: true, error: None, data: None, len }
    }
}

pub struct Channel {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Channel {
    pub fn new(stream: UnixStream) -> std::io::Result<Self> {
        let writer = stream.try_clone()?;
        Ok(Channel { reader: BufReader::new(stream), writer })
    }

    pub fn send<T: Serialize>(&mut self, msg: &T, payload: &[u8]) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(msg)?;
        line.push(b'\n');
        self.writer.write_all(&line)?;
        if !payload.is_empty() {
            self.writer.write_all(payload)?;
        }
        self.writer.flush()
    }

    pub fn recv<T: for<'de> Deserialize<'de>>(&mut self) -> std::io::Result<(T, Vec<u8>)> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line)?;
        if n == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "helper closed"));
        }
        let msg: T = serde_json::from_str(line.trim_end())?;
        let len = extract_len(&line);
        let mut payload = vec![0u8; len];
        if len > 0 {
            self.reader.read_exact(&mut payload)?;
        }
        Ok((msg, payload))
    }
}

/// Both Request and Response carry `len`; read it back without knowing which.
fn extract_len(line: &str) -> usize {
    serde_json::from_str::<serde_json::Value>(line.trim_end())
        .ok()
        .and_then(|v| v.get("len").and_then(|l| l.as_u64()))
        .unwrap_or(0) as usize
}
