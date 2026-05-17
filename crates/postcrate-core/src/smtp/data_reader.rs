//! Streaming DATA reader.
//!
//! - Reads CRLF-terminated lines until `.\r\n`.
//! - Un-dot-stuffs leading `.` (RFC 5321 §4.5.2).
//! - Enforces the 1000-octet line limit.
//! - Tracks total bytes; aborts past `max_bytes` with `SizeExceeded`.
//! - Spills to a temp file once `spill_at` is crossed so very large
//!   messages never live entirely in memory.

use std::path::{Path, PathBuf};

use bytes::BytesMut;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt};

use crate::error::{Error, Result};
use crate::smtp::codec::LineReader;

/// Outcome of a DATA read.
#[derive(Debug)]
pub enum DataOutcome {
    Complete(CapturedSource),
    SizeExceeded,
    Eof,
}

#[derive(Debug)]
pub enum CapturedSource {
    InMemory(BytesMut),
    OnDisk(PathBuf, u64),
}

impl CapturedSource {
    pub fn size_bytes(&self) -> u64 {
        match self {
            CapturedSource::InMemory(b) => b.len() as u64,
            CapturedSource::OnDisk(_, n) => *n,
        }
    }
}

pub struct DataReadCfg {
    pub max_line: usize,
    pub max_bytes: u64,
    pub spill_at: usize,
    pub spill_dir: PathBuf,
}

/// Drive the DATA phase. The caller has already sent `354`. We return
/// when we see the terminating `.\r\n`, or when size/connection limits
/// are hit, or on EOF.
pub async fn read_data<R: AsyncRead + Unpin>(
    reader: &mut LineReader<R>,
    cfg: &DataReadCfg,
) -> Result<DataOutcome> {
    let mut mem = BytesMut::with_capacity(cfg.spill_at.min(64 * 1024));
    let mut spill: Option<(File, PathBuf)> = None;
    let mut total: u64 = 0;

    loop {
        let line = read_data_line(reader, cfg.max_line).await?;
        let line = match line {
            Some(l) => l,
            None => return Ok(DataOutcome::Eof),
        };

        if line.as_slice() == b"." {
            // End of DATA.
            return Ok(DataOutcome::Complete(finalize(mem, spill, total).await?));
        }

        // Un-dot-stuff: a single leading `.` is collapsed.
        let body: &[u8] = if line.first() == Some(&b'.') {
            &line[1..]
        } else {
            &line
        };

        let line_len = body.len() as u64 + 2; // include CRLF in size accounting
        if total + line_len > cfg.max_bytes {
            // Drain the rest cheaply so the client gets a clean reply.
            return Ok(DataOutcome::SizeExceeded);
        }
        total += line_len;

        if let Some((file, _)) = spill.as_mut() {
            file.write_all(body).await?;
            file.write_all(b"\r\n").await?;
        } else if mem.len() + body.len() + 2 > cfg.spill_at {
            // Promote to disk.
            tokio::fs::create_dir_all(&cfg.spill_dir).await?;
            let path = cfg
                .spill_dir
                .join(format!("{}.tmp", uuid::Uuid::new_v4()));
            let mut f = File::create(&path).await?;
            f.write_all(&mem).await?;
            f.write_all(body).await?;
            f.write_all(b"\r\n").await?;
            spill = Some((f, path));
            mem = BytesMut::new();
        } else {
            mem.extend_from_slice(body);
            mem.extend_from_slice(b"\r\n");
        }
    }
}

async fn finalize(
    mem: BytesMut,
    spill: Option<(File, PathBuf)>,
    total: u64,
) -> Result<CapturedSource> {
    if let Some((mut f, path)) = spill {
        f.flush().await?;
        drop(f);
        Ok(CapturedSource::OnDisk(path, total))
    } else {
        Ok(CapturedSource::InMemory(mem))
    }
}

async fn read_data_line<R: AsyncRead + Unpin>(
    reader: &mut LineReader<R>,
    max_line: usize,
) -> Result<Option<Vec<u8>>> {
    let inner = reader.as_buf_mut();
    let mut buf = Vec::with_capacity(128);

    loop {
        let chunk = inner.fill_buf().await?;
        if chunk.is_empty() {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Err(Error::SmtpProto("DATA unterminated at EOF".into()))
            };
        }
        let mut consumed = 0;
        for (i, b) in chunk.iter().enumerate() {
            consumed = i + 1;
            if *b == b'\n' {
                if let Some(prev) = buf.last() {
                    if *prev == b'\r' {
                        buf.pop();
                    }
                }
                inner.consume(consumed);
                if buf.len() > max_line {
                    return Err(Error::SmtpProto("DATA line too long".into()));
                }
                return Ok(Some(buf));
            }
            buf.push(*b);
            if buf.len() > max_line {
                inner.consume(consumed);
                return Err(Error::SmtpProto("DATA line too long".into()));
            }
        }
        inner.consume(consumed);
    }
}

/// Move a spilled tempfile into its final destination directory. The
/// returned path is what we store in `emails.raw_path`.
pub async fn finalize_to_blob(
    captured: &CapturedSource,
    blob_dir: &Path,
    email_id: &str,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(blob_dir).await?;
    let final_path = blob_dir.join(format!("{email_id}.eml"));
    match captured {
        CapturedSource::InMemory(bytes) => {
            tokio::fs::write(&final_path, bytes).await?;
        }
        CapturedSource::OnDisk(temp_path, _) => match tokio::fs::rename(temp_path, &final_path).await {
            Ok(()) => {}
            Err(_) => {
                // Cross-device rename can fail — copy + delete.
                tokio::fs::copy(temp_path, &final_path).await?;
                let _ = tokio::fs::remove_file(temp_path).await;
            }
        },
    }
    Ok(final_path)
}

/// Load the captured bytes regardless of where they ended up.
pub async fn load_bytes(captured: &CapturedSource) -> Result<Vec<u8>> {
    match captured {
        CapturedSource::InMemory(b) => Ok(b.to_vec()),
        CapturedSource::OnDisk(path, _) => Ok(tokio::fs::read(path).await?),
    }
}
