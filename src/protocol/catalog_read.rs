//! `tdf/catalog/1` ALPN — reader-side CWT-gated catalog stream.
//!
//! Wire format: length-prefixed CBOR frames over a single bidi QUIC stream.
//! Reader sends one `CatalogSubscribe`, then receives a sequence of
//! `CatalogStreamMsg` frames (Entry / CaughtUp / Heartbeat /
//! TokenExpiringSoon / Error) until close.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::catalog::types::ContentEvent;

pub const ALPN: &[u8] = b"tdf/catalog/1";

const MAX_REQUEST_BYTES: u32 = 64 * 1024;
const MAX_FRAME_BYTES: u32 = 256 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct CatalogSubscribe {
    pub cwt: ByteBuf,
    pub after_seq: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CatalogStreamMsg {
    Entry(ContentEvent),
    CaughtUp { seq: u64 },
    Heartbeat,
    TokenExpiringSoon { exp: i64 },
    Error { code: ErrorCode, message: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ErrorCode {
    BadRequest,
    PdpUnavailable,
    Internal,
    TooManySubscriptions,
}

pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    msg: &T,
) -> Result<()> {
    let mut buf = Vec::with_capacity(256);
    ciborium::ser::into_writer(msg, &mut buf).context("encode frame as CBOR")?;
    if buf.len() as u32 > MAX_FRAME_BYTES {
        bail!("frame too large ({} bytes)", buf.len());
    }
    w.write_u32(buf.len() as u32).await.context("write frame length")?;
    w.write_all(&buf).await.context("write frame body")?;
    w.flush().await.ok();
    Ok(())
}

pub async fn read_request<R: AsyncRead + Unpin>(r: &mut R) -> Result<CatalogSubscribe> {
    let len = r.read_u32().await.context("read request length")?;
    if len > MAX_REQUEST_BYTES {
        bail!("request too large ({len} bytes)");
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await.context("read request body")?;
    let req: CatalogSubscribe = ciborium::de::from_reader(buf.as_slice())
        .context("decode CatalogSubscribe CBOR")?;
    Ok(req)
}
