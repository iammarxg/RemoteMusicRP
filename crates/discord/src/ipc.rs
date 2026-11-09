// SPDX-License-Identifier: AGPL-3.0-only
//! Discord IPC framing: `opcode: u32 LE | length: u32 LE | JSON payload`.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{DiscordError, Result};

/// Refuse frames larger than this (Discord replies are a few KiB).
pub const MAX_FRAME: u32 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Handshake = 0,
    Frame = 1,
    Close = 2,
    Ping = 3,
    Pong = 4,
}

impl Opcode {
    fn from_u32(v: u32) -> Result<Self> {
        Ok(match v {
            0 => Self::Handshake,
            1 => Self::Frame,
            2 => Self::Close,
            3 => Self::Ping,
            4 => Self::Pong,
            other => return Err(DiscordError::Protocol(format!("unknown opcode {other}"))),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub op: Opcode,
    pub payload: Vec<u8>,
}

pub fn encode(op: Opcode, payload: &[u8]) -> Result<Vec<u8>> {
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|l| *l <= MAX_FRAME)
        .ok_or_else(|| DiscordError::Protocol("frame too large".into()))?;
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&(op as u32).to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    op: Opcode,
    payload: &[u8],
) -> Result<()> {
    w.write_all(&encode(op, payload)?).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Frame> {
    let mut header = [0u8; 8];
    r.read_exact(&mut header).await?;
    let op = Opcode::from_u32(u32::from_le_bytes([
        header[0], header[1], header[2], header[3],
    ]))?;
    let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    if len > MAX_FRAME {
        return Err(DiscordError::Protocol("frame too large".into()));
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).await?;
    Ok(Frame { op, payload })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn handshake_matches_documented_bytes() {
        // Docs example: opcode 0, length 45 for this exact payload.
        let payload = br#"{"v":1,"client_id":"192741864418312192"}"#;
        let bytes = encode(Opcode::Handshake, payload).unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 0, 0]);
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
            payload.len()
        );
    }

    #[tokio::test]
    async fn roundtrip_over_duplex() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_frame(&mut a, Opcode::Frame, b"{\"x\":1}")
            .await
            .unwrap();
        let f = read_frame(&mut b).await.unwrap();
        assert_eq!(f.op, Opcode::Frame);
        assert_eq!(f.payload, b"{\"x\":1}");
    }

    #[tokio::test]
    async fn rejects_oversized_and_unknown() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let mut hdr = Vec::new();
        hdr.extend_from_slice(&1u32.to_le_bytes());
        hdr.extend_from_slice(&(MAX_FRAME + 1).to_le_bytes());
        a.write_all(&hdr).await.unwrap();
        assert!(matches!(
            read_frame(&mut b).await,
            Err(DiscordError::Protocol(_))
        ));

        let (mut a, mut b) = tokio::io::duplex(64);
        let mut hdr = Vec::new();
        hdr.extend_from_slice(&9u32.to_le_bytes());
        hdr.extend_from_slice(&0u32.to_le_bytes());
        a.write_all(&hdr).await.unwrap();
        assert!(matches!(
            read_frame(&mut b).await,
            Err(DiscordError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn truncated_frame_is_io_error() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let mut bytes = encode(Opcode::Frame, b"0123456789").unwrap();
        bytes.truncate(12);
        a.write_all(&bytes).await.unwrap();
        drop(a);
        assert!(matches!(read_frame(&mut b).await, Err(DiscordError::Io(_))));
    }
}
