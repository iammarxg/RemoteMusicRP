// SPDX-License-Identifier: AGPL-3.0-only
//! Local RPC client: connect, handshake, SET_ACTIVITY.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::activity::Activity;
use crate::ipc::{Opcode, read_frame, write_frame};
use crate::{DiscordError, Result, valid_client_id};

const OP_TIMEOUT: Duration = Duration::from_secs(5);

/// Any bidirectional byte stream (named pipe, Unix socket, test duplex).
pub trait IpcStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> IpcStream for T {}

/// The Discord user the client is logged in as (from READY). Display only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedUser {
    pub id: String,
    pub username: String,
    /// Display name ("global name"), if set.
    pub display_name: Option<String>,
    /// Avatar hash (validated: hex, optional `a_` prefix for animated).
    pub avatar: Option<String>,
}

impl ConnectedUser {
    /// Public CDN URL for the avatar, if any.
    pub fn avatar_url(&self) -> Option<String> {
        let hash = self.avatar.as_deref()?;
        Some(format!(
            "https://cdn.discordapp.com/avatars/{}/{hash}.png?size=128",
            self.id
        ))
    }
}

impl ConnectedUser {
    /// Build from untrusted parts, with the same validation for every source
    /// (local RPC READY payload, Social SDK): numeric id, control characters
    /// removed, lengths capped, avatar hash must be hex (optional `a_`).
    pub fn from_parts(
        id: &str,
        username: &str,
        global_name: Option<&str>,
        avatar: Option<&str>,
    ) -> Option<Self> {
        let id: String = id.chars().take(20).collect();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let text = |s: &str, n: usize| -> Option<String> {
            Some(
                s.chars()
                    .filter(|c| !c.is_control())
                    .take(n)
                    .collect::<String>(),
            )
            .filter(|s| !s.trim().is_empty())
        };
        let avatar = avatar.and_then(|a| text(a, 40)).filter(|h| {
            let hex = h.strip_prefix("a_").unwrap_or(h);
            !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit())
        });
        Some(Self {
            id,
            username: text(username, 64)?,
            display_name: global_name.and_then(|g| text(g, 64)),
            avatar,
        })
    }
}

fn parse_user(u: &Value) -> Option<ConnectedUser> {
    let s = |k: &str| u.get(k).and_then(Value::as_str);
    ConnectedUser::from_parts(s("id")?, s("username")?, s("global_name"), s("avatar"))
}

pub struct RpcClient {
    stream: Box<dyn IpcStream>,
    user: Option<ConnectedUser>,
}

impl std::fmt::Debug for RpcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcClient")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

fn nonce() -> String {
    let mut b = [0u8; 16];
    // A nonce only needs uniqueness; fall back to time on RNG failure.
    if getrandom::fill(&mut b).is_err() {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        b = t.to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

async fn with_timeout<T>(fut: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(OP_TIMEOUT, fut)
        .await
        .map_err(|_| DiscordError::Timeout)?
}

fn parse_json(payload: &[u8]) -> Result<Value> {
    serde_json::from_slice(payload)
        .map_err(|_| DiscordError::Protocol("invalid JSON from Discord".into()))
}

fn close_reason(payload: &[u8]) -> String {
    parse_json(payload)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(Value::as_str)
                .map(|s| s.chars().take(200).collect())
        })
        .unwrap_or_else(|| "no reason given".into())
}

impl RpcClient {
    /// Connect to the first reachable Discord client and complete the handshake.
    pub async fn connect(client_id: &str) -> Result<Self> {
        if !valid_client_id(client_id) {
            return Err(DiscordError::InvalidClientId);
        }
        let stream = transport::open().await?;
        Self::handshake(stream, client_id).await
    }

    /// Handshake over an already-open stream (also used by tests).
    pub async fn handshake(stream: Box<dyn IpcStream>, client_id: &str) -> Result<Self> {
        if !valid_client_id(client_id) {
            return Err(DiscordError::InvalidClientId);
        }
        let mut client = Self { stream, user: None };
        let hello = json!({ "v": 1, "client_id": client_id }).to_string();
        with_timeout(async {
            write_frame(&mut client.stream, Opcode::Handshake, hello.as_bytes()).await?;
            loop {
                let frame = read_frame(&mut client.stream).await?;
                match frame.op {
                    Opcode::Frame => {
                        let v = parse_json(&frame.payload)?;
                        if v.get("evt").and_then(Value::as_str) == Some("READY") {
                            client.user = v.pointer("/data/user").and_then(parse_user);
                            return Ok(());
                        }
                        if v.get("evt").and_then(Value::as_str) == Some("ERROR") {
                            return Err(rejected(&v));
                        }
                    }
                    Opcode::Ping => {
                        write_frame(&mut client.stream, Opcode::Pong, &frame.payload).await?
                    }
                    Opcode::Close => {
                        return Err(DiscordError::Closed(close_reason(&frame.payload)));
                    }
                    _ => {}
                }
            }
        })
        .await?;
        Ok(client)
    }

    pub fn user(&self) -> Option<&ConnectedUser> {
        self.user.as_ref()
    }

    /// Set (Some) or clear (None) the activity.
    pub async fn set_activity(&mut self, activity: Option<&Activity>) -> Result<()> {
        let n = nonce();
        let cmd = json!({
            "cmd": "SET_ACTIVITY",
            "args": {
                "pid": std::process::id(),
                "activity": activity.map(Activity::to_json),
            },
            "nonce": n,
        })
        .to_string();
        let stream = &mut self.stream;
        with_timeout(async move {
            write_frame(stream, Opcode::Frame, cmd.as_bytes()).await?;
            loop {
                let frame = read_frame(stream).await?;
                match frame.op {
                    Opcode::Frame => {
                        let v = parse_json(&frame.payload)?;
                        if v.get("nonce").and_then(Value::as_str) != Some(n.as_str()) {
                            continue; // unrelated event
                        }
                        if v.get("evt").and_then(Value::as_str) == Some("ERROR") {
                            return Err(rejected(&v));
                        }
                        return Ok(());
                    }
                    Opcode::Ping => write_frame(stream, Opcode::Pong, &frame.payload).await?,
                    Opcode::Close => {
                        return Err(DiscordError::Closed(close_reason(&frame.payload)));
                    }
                    _ => {}
                }
            }
        })
        .await
    }

    /// Politely close the connection.
    pub async fn close(mut self) {
        let _ = write_frame(&mut self.stream, Opcode::Close, b"{}").await;
    }
}

fn rejected(v: &Value) -> DiscordError {
    DiscordError::Rejected {
        code: v.pointer("/data/code").and_then(Value::as_i64).unwrap_or(0),
        message: v
            .pointer("/data/message")
            .and_then(Value::as_str)
            .map(|s| s.chars().take(200).collect())
            .unwrap_or_default(),
    }
}

#[cfg(windows)]
mod transport {
    use std::time::Duration;

    use tokio::net::windows::named_pipe::ClientOptions;

    use super::IpcStream;
    use crate::paths::windows_pipes;
    use crate::{DiscordError, Result};

    const ERROR_PIPE_BUSY: i32 = 231;

    pub async fn open() -> Result<Box<dyn IpcStream>> {
        for name in windows_pipes() {
            for _attempt in 0..3 {
                match ClientOptions::new().open(&name) {
                    Ok(pipe) => return Ok(Box::new(pipe)),
                    Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    Err(_) => break,
                }
            }
        }
        Err(DiscordError::NotRunning)
    }
}

#[cfg(unix)]
mod transport {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    use tokio::net::UnixStream;

    use super::IpcStream;
    use crate::paths::unix_candidates;
    use crate::{DiscordError, Result};

    /// Owner uid of our own runtime/home directory, used to skip sockets
    /// planted by other users in shared directories such as /tmp.
    fn expected_uid() -> Option<u32> {
        let home = std::env::var_os("HOME")?;
        std::fs::metadata(home).ok().map(|m| m.uid())
    }

    pub async fn open() -> Result<Box<dyn IpcStream>> {
        let (_base, candidates) = unix_candidates(|k| std::env::var(k).ok());
        let uid = expected_uid();
        for path in candidates {
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_socket() {
                continue;
            }
            if uid.is_some_and(|u| meta.uid() != u) {
                tracing::warn!("skipping Discord IPC socket owned by another user");
                continue;
            }
            if let Ok(s) = UnixStream::connect(&path).await {
                return Ok(Box::new(s));
            }
        }
        Err(DiscordError::NotRunning)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::ipc::{read_frame, write_frame};

    const ID: &str = "1234567890123456789";

    /// Minimal fake Discord: answers handshake with READY, then echoes the
    /// nonce of each command (or an ERROR if `reject` is set).
    async fn fake_discord(mut s: tokio::io::DuplexStream, reject: bool) -> Vec<Value> {
        let mut seen = Vec::new();
        let hs = read_frame(&mut s).await.unwrap();
        assert_eq!(hs.op, Opcode::Handshake);
        seen.push(parse_json(&hs.payload).unwrap());
        // An unrelated ping first, which the client must answer.
        write_frame(&mut s, Opcode::Ping, b"{}").await.unwrap();
        assert_eq!(read_frame(&mut s).await.unwrap().op, Opcode::Pong);
        let ready = json!({"cmd":"DISPATCH","evt":"READY","data":{"v":1,"user":{"id":"42","username":"listener","global_name":"Listener","avatar":"a_0123abcd"}}});
        write_frame(&mut s, Opcode::Frame, ready.to_string().as_bytes())
            .await
            .unwrap();
        while let Ok(f) = read_frame(&mut s).await {
            if f.op == Opcode::Close {
                break;
            }
            let v = parse_json(&f.payload).unwrap();
            let nonce = v["nonce"].clone();
            seen.push(v);
            let reply = if reject {
                json!({"cmd":"SET_ACTIVITY","evt":"ERROR","nonce":nonce,"data":{"code":4000,"message":"bad"}})
            } else {
                json!({"cmd":"SET_ACTIVITY","evt":null,"nonce":nonce,"data":{}})
            };
            // Send an unrelated event first; client must skip it.
            write_frame(&mut s, Opcode::Frame, br#"{"evt":"OTHER","nonce":"x"}"#)
                .await
                .unwrap();
            write_frame(&mut s, Opcode::Frame, reply.to_string().as_bytes())
                .await
                .unwrap();
        }
        seen
    }

    #[tokio::test]
    async fn handshake_set_and_clear() {
        let (client_side, server_side) = tokio::io::duplex(8192);
        let server = tokio::spawn(fake_discord(server_side, false));
        let mut c = RpcClient::handshake(Box::new(client_side), ID)
            .await
            .unwrap();
        let u = c.user().unwrap();
        assert_eq!(u.username, "listener");
        assert_eq!(u.display_name.as_deref(), Some("Listener"));
        assert_eq!(
            u.avatar_url().as_deref(),
            Some("https://cdn.discordapp.com/avatars/42/a_0123abcd.png?size=128")
        );
        let act = Activity {
            details: Some("Song".into()),
            state: Some("Artist".into()),
            ..Default::default()
        };
        c.set_activity(Some(&act)).await.unwrap();
        c.set_activity(None).await.unwrap();
        c.close().await;
        let seen = server.await.unwrap();
        assert_eq!(seen[0]["client_id"], ID);
        assert_eq!(seen[1]["cmd"], "SET_ACTIVITY");
        assert_eq!(seen[1]["args"]["activity"]["type"], 2);
        assert_eq!(seen[1]["args"]["activity"]["details"], "Song");
        assert!(seen[2]["args"]["activity"].is_null());
    }

    #[test]
    fn user_parsing_rejects_bad_values() {
        let bad_avatar = json!({"id":"42","username":"u","avatar":"../../evil"});
        assert_eq!(parse_user(&bad_avatar).unwrap().avatar, None);
        assert!(parse_user(&json!({"id":"x","username":"u"})).is_none());
        assert!(parse_user(&json!({"id":"42"})).is_none());
    }

    #[tokio::test]
    async fn rejection_is_reported() {
        let (client_side, server_side) = tokio::io::duplex(8192);
        tokio::spawn(fake_discord(server_side, true));
        let mut c = RpcClient::handshake(Box::new(client_side), ID)
            .await
            .unwrap();
        let err = c
            .set_activity(Some(&Activity::default()))
            .await
            .unwrap_err();
        assert!(matches!(err, DiscordError::Rejected { code: 4000, .. }));
    }

    #[tokio::test]
    async fn close_during_handshake() {
        let (client_side, mut server_side) = tokio::io::duplex(8192);
        tokio::spawn(async move {
            let _ = read_frame(&mut server_side).await;
            write_frame(
                &mut server_side,
                Opcode::Close,
                br#"{"code":4000,"message":"Invalid Client ID"}"#,
            )
            .await
            .unwrap();
        });
        let err = RpcClient::handshake(Box::new(client_side), ID)
            .await
            .unwrap_err();
        assert!(matches!(err, DiscordError::Closed(m) if m == "Invalid Client ID"));
    }

    #[tokio::test]
    async fn rejects_bad_client_id() {
        let (client_side, _server) = tokio::io::duplex(64);
        assert!(matches!(
            RpcClient::handshake(Box::new(client_side), "abc").await,
            Err(DiscordError::InvalidClientId)
        ));
    }
}
