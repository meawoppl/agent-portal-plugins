//! KiCad 9 IPC client (JSON envelopes over an NNG SP Req0 socket).
//!
//! This mirrors the transport used by upstream kicad-tools while remaining a
//! native Rust implementation.  Mutations are grouped in KiCad's undo
//! transaction protocol and rolled back on any failed operation.

#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(unix)]
const REQ0_PROTOCOL: u16 = 0x30;
#[cfg(unix)]
const REP0_PROTOCOL: u16 = 0x31;
#[cfg(unix)]
const SP_MESSAGE: u8 = 0x01;
#[cfg(unix)]
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

pub struct Client {
    #[cfg(unix)]
    stream: UnixStream,
    #[cfg(unix)]
    next_request_id: u32,
    token: String,
}

impl Client {
    #[cfg(unix)]
    pub fn connect(path: &Path) -> Result<Self> {
        let mut stream = UnixStream::connect(path)
            .with_context(|| format!("connecting to KiCad IPC at {}", path.display()))?;
        stream.set_read_timeout(Some(DEFAULT_TIMEOUT))?;
        stream.set_write_timeout(Some(DEFAULT_TIMEOUT))?;
        negotiate(&mut stream).context("negotiating NNG SP Req0 transport")?;
        Ok(Self {
            stream,
            next_request_id: 0x8000_0001,
            token: String::new(),
        })
    }

    #[cfg(not(unix))]
    pub fn connect(path: &Path) -> Result<Self> {
        bail!(
            "KiCad IPC uses a Unix-domain socket, unsupported on this platform: {}",
            path.display()
        )
    }

    #[cfg(unix)]
    pub fn request(&mut self, command: &str, params: Value) -> Result<Value> {
        let mut request = json!({"command": command});
        if !self.token.is_empty() {
            request["token"] = Value::String(self.token.clone());
        }
        if params.as_object().is_some_and(|m| !m.is_empty()) {
            request["params"] = params;
        }
        let bytes = serde_json::to_vec(&request)?;
        let request_id = self.take_request_id();
        write_message(&mut self.stream, request_id, &bytes)
            .with_context(|| format!("sending KiCad IPC command {command}"))?;
        let (response_id, response) = read_message(&mut self.stream)
            .with_context(|| format!("waiting for KiCad IPC response to {command}"))?;
        if response_id != request_id {
            bail!(
                "KiCad IPC reply request ID mismatch: sent {request_id:#010x}, received {response_id:#010x}"
            );
        }
        let response: Value = serde_json::from_slice(&response)
            .with_context(|| format!("decoding KiCad IPC response to {command}"))?;
        let status = response.get("status").and_then(Value::as_i64).unwrap_or(0);
        if status != 0 {
            bail!(
                "KiCad API error ({status}): {}",
                response
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            );
        }
        let result = response.get("result").cloned().unwrap_or_else(|| json!({}));
        if let Some(token) = result.get("token").and_then(Value::as_str) {
            self.token = token.to_owned();
        }
        Ok(result)
    }

    #[cfg(not(unix))]
    pub fn request(&mut self, command: &str, _params: Value) -> Result<Value> {
        bail!("KiCad IPC command {command} is unsupported on this platform")
    }

    #[cfg(unix)]
    fn take_request_id(&mut self) -> u32 {
        let id = self.next_request_id | 0x8000_0000;
        self.next_request_id = self.next_request_id.wrapping_add(1) | 0x8000_0000;
        id
    }

    pub fn ping(&mut self) -> Result<bool> {
        #[cfg(unix)]
        self.stream.set_read_timeout(Some(HEALTH_TIMEOUT))?;
        let result = self.request("Ping", json!({}));
        #[cfg(unix)]
        self.stream.set_read_timeout(Some(DEFAULT_TIMEOUT))?;
        result.map(|_| true)
    }

    pub fn version(&mut self) -> Result<String> {
        Ok(self.request("GetVersion", json!({}))?["version"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned())
    }

    pub fn open_documents(&mut self) -> Result<Vec<Value>> {
        Ok(self.request("GetOpenDocuments", json!({}))?["documents"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    pub fn create_items_transaction(
        &mut self,
        description: &str,
        items: Vec<Value>,
    ) -> Result<Vec<String>> {
        self.request("BeginCommit", json!({"description": description}))?;
        let created = self.request("CreateItems", json!({"items": items}));
        match created {
            Ok(result) => {
                if let Err(error) = self.request("PushCommit", json!({})) {
                    let _ = self.request("DropCommit", json!({}));
                    return Err(error).context("committing KiCad IPC transaction");
                }
                Ok(result["created_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect())
            }
            Err(error) => {
                let _ = self.request("DropCommit", json!({}));
                Err(error).context("creating KiCad board items")
            }
        }
    }
}

#[cfg(unix)]
fn negotiate(stream: &mut UnixStream) -> Result<()> {
    let mut ours = [0_u8, b'S', b'P', 0, 0, 0, 0, 0];
    ours[4..6].copy_from_slice(&REQ0_PROTOCOL.to_be_bytes());
    stream.write_all(&ours)?;

    let mut peer = [0_u8; 8];
    stream.read_exact(&mut peer)?;
    if peer[..4] != [0, b'S', b'P', 0] || peer[6..] != [0, 0] {
        bail!("invalid NNG SP handshake: {peer:02x?}");
    }
    let protocol = u16::from_be_bytes([peer[4], peer[5]]);
    if protocol != REP0_PROTOCOL {
        bail!(
            "KiCad IPC peer offered protocol {protocol:#04x}, expected REP0 ({REP0_PROTOCOL:#04x})"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn write_message(stream: &mut UnixStream, request_id: u32, body: &[u8]) -> Result<()> {
    let length = u64::try_from(body.len())?
        .checked_add(4)
        .context("KiCad IPC request is too large")?;
    let mut header = [0_u8; 9];
    header[0] = SP_MESSAGE;
    header[1..].copy_from_slice(&length.to_be_bytes());
    stream.write_all(&header)?;
    stream.write_all(&request_id.to_be_bytes())?;
    stream.write_all(body)?;
    Ok(())
}

#[cfg(unix)]
fn read_message(stream: &mut UnixStream) -> Result<(u32, Vec<u8>)> {
    let mut header = [0_u8; 9];
    stream.read_exact(&mut header)?;
    if header[0] != SP_MESSAGE {
        bail!("invalid NNG SP message type {:#04x}", header[0]);
    }
    let length = u64::from_be_bytes(header[1..].try_into().expect("fixed-size slice"));
    if !(4..=MAX_RESPONSE_BYTES).contains(&length) {
        bail!("invalid KiCad IPC response length {length}");
    }
    let mut request_id = [0_u8; 4];
    stream.read_exact(&mut request_id)?;
    let body_length = usize::try_from(length - 4)?;
    let mut body = vec![0_u8; body_length];
    stream.read_exact(&mut body)?;
    Ok((u32::from_be_bytes(request_id), body))
}

pub fn discover_sockets(explicit: Option<&Path>) -> Vec<PathBuf> {
    if let Some(path) = explicit {
        return path
            .exists()
            .then(|| path.to_path_buf())
            .into_iter()
            .collect();
    }
    let mut roots = Vec::new();
    for key in ["KICAD_IPC_SOCKET", "KICAD_API_SOCKET"] {
        if let Some(value) = std::env::var_os(key) {
            roots.push(PathBuf::from(value));
        }
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        roots.push(PathBuf::from(runtime));
    }
    roots.push(PathBuf::from("/tmp"));
    let mut out = Vec::new();
    for root in roots {
        if root.is_file() {
            out.push(root);
            continue;
        }
        visit_socket_candidates(&root, 0, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn visit_socket_candidates(root: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    if depth > 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_socket_candidates(&path, depth + 1, out);
            continue;
        }
        let name = path.to_string_lossy().to_ascii_lowercase();
        if name.contains("kicad") && (name.ends_with(".sock") || name.contains("api")) {
            out.push(path);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;

    fn accept_rep0(listener: UnixListener) -> UnixStream {
        let (mut stream, _) = listener.accept().unwrap();
        let mut handshake = [0_u8; 8];
        stream.read_exact(&mut handshake).unwrap();
        assert_eq!(handshake, [0, b'S', b'P', 0, 0, 0x30, 0, 0]);
        stream
            .write_all(&[0, b'S', b'P', 0, 0, 0x31, 0, 0])
            .unwrap();
        stream
    }

    fn receive_request(stream: &mut UnixStream) -> (u32, Value) {
        let (id, body) = read_message(stream).unwrap();
        assert_ne!(id & 0x8000_0000, 0, "REQ0 request ID needs its high bit");
        (id, serde_json::from_slice(&body).unwrap())
    }

    fn send_response(stream: &mut UnixStream, id: u32, result: Value) {
        let body = serde_json::to_vec(&json!({"status":0,"result":result})).unwrap();
        write_message(stream, id, &body).unwrap();
    }

    #[test]
    fn negotiated_transaction_captures_token_and_commits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kicad-api.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let worker = thread::spawn(move || {
            let mut server = accept_rep0(listener);
            let expected = ["BeginCommit", "CreateItems", "PushCommit"];
            for (index, command) in expected.iter().enumerate() {
                let (id, request) = receive_request(&mut server);
                assert_eq!(request["command"], *command);
                if index > 0 {
                    assert_eq!(request["token"], "negotiated-token")
                }
                let result = if index == 0 {
                    json!({"token":"negotiated-token"})
                } else if index == 1 {
                    json!({"created_ids":["a","b"]})
                } else {
                    json!({})
                };
                send_response(&mut server, id, result);
            }
        });
        let mut client = Client::connect(&path).unwrap();
        let ids = client
            .create_items_transaction("test", vec![json!({"type":"via"})])
            .unwrap();
        assert_eq!(ids, ["a", "b"]);
        worker.join().unwrap();
    }

    #[test]
    fn rejects_reply_for_a_different_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kicad-api.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let worker = thread::spawn(move || {
            let mut server = accept_rep0(listener);
            let (id, _) = receive_request(&mut server);
            send_response(&mut server, id.wrapping_add(1), json!({}));
        });
        let error = Client::connect(&path)
            .unwrap()
            .request("Ping", json!({}))
            .unwrap_err();
        assert!(error.to_string().contains("request ID mismatch"));
        worker.join().unwrap();
    }
}
