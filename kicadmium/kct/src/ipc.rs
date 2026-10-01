//! KiCad 9 IPC client (JSON envelopes over an NNG Req0 socket).
//!
//! This mirrors the transport used by upstream kicad-tools while remaining a
//! native Rust implementation.  Mutations are grouped in KiCad's undo
//! transaction protocol and rolled back on any failed operation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use nng::options::{Options, RecvTimeout, SendTimeout};
use nng::{Protocol, Socket};
use serde_json::{json, Value};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Client {
    socket: Socket,
    token: String,
}

impl Client {
    pub fn connect(path: &Path) -> Result<Self> {
        let socket = Socket::new(Protocol::Req0).context("creating NNG request socket")?;
        socket.set_opt::<RecvTimeout>(Some(DEFAULT_TIMEOUT))?;
        socket.set_opt::<SendTimeout>(Some(DEFAULT_TIMEOUT))?;
        socket
            .dial(&format!("ipc://{}", path.display()))
            .with_context(|| format!("connecting to KiCad IPC at {}", path.display()))?;
        Ok(Self {
            socket,
            token: String::new(),
        })
    }

    pub fn request(&mut self, command: &str, params: Value) -> Result<Value> {
        let mut request = json!({"command": command});
        if !self.token.is_empty() {
            request["token"] = Value::String(self.token.clone());
        }
        if params.as_object().is_some_and(|m| !m.is_empty()) {
            request["params"] = params;
        }
        let bytes = serde_json::to_vec(&request)?;
        self.socket
            .send(bytes.as_slice())
            .map_err(|(_, e)| e)
            .with_context(|| format!("sending KiCad IPC command {command}"))?;
        let response = self
            .socket
            .recv()
            .with_context(|| format!("waiting for KiCad IPC response to {command}"))?;
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

    pub fn ping(&mut self) -> Result<bool> {
        self.socket.set_opt::<RecvTimeout>(Some(HEALTH_TIMEOUT))?;
        let result = self.request("Ping", json!({}));
        self.socket.set_opt::<RecvTimeout>(Some(DEFAULT_TIMEOUT))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn negotiated_transaction_captures_token_and_commits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kicad-api.sock");
        let address = format!("ipc://{}", path.display());
        let server = Socket::new(Protocol::Rep0).unwrap();
        server.listen(&address).unwrap();
        let worker = thread::spawn(move || {
            let expected = ["BeginCommit", "CreateItems", "PushCommit"];
            for (index, command) in expected.iter().enumerate() {
                let message = server.recv().unwrap();
                let request: Value = serde_json::from_slice(&message).unwrap();
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
                let response = serde_json::to_vec(&json!({"status":0,"result":result})).unwrap();
                server.send(response.as_slice()).unwrap();
            }
        });
        let mut client = Client::connect(&path).unwrap();
        let ids = client
            .create_items_transaction("test", vec![json!({"type":"via"})])
            .unwrap();
        assert_eq!(ids, ["a", "b"]);
        worker.join().unwrap();
    }
}
