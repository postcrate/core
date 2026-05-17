//! Tiny line reader. We don't want a full framed codec — SMTP framing
//! varies by phase (line-mode in command phase, byte-mode + special
//! dot-stuffing rules during DATA), so each phase reads what it needs.

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use crate::error::{Error, Result};

/// Buffered reader specialized for SMTP's command phase.
pub struct LineReader<R> {
    inner: BufReader<R>,
    max_line: usize,
    bytes_read: u64,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub fn new(r: R, max_line: usize) -> Self {
        Self {
            inner: BufReader::with_capacity(8 * 1024, r),
            max_line,
            bytes_read: 0,
        }
    }

    pub fn into_inner(self) -> R {
        self.inner.into_inner()
    }

    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Read one CRLF-terminated line. Returns `Ok(None)` at clean EOF.
    /// Errors with [`Error::SmtpProto`] if the line exceeds `max_line`.
    pub async fn next_line(&mut self) -> Result<Option<String>> {
        let mut buf = Vec::with_capacity(128);
        let mut total = 0usize;

        loop {
            let chunk = self.inner.fill_buf().await?;
            if chunk.is_empty() {
                if buf.is_empty() {
                    return Ok(None);
                }
                // Partial line at EOF — treat as a protocol error.
                return Err(Error::SmtpProto("unterminated line at EOF".into()));
            }

            let mut consumed = 0;
            for (i, b) in chunk.iter().enumerate() {
                consumed = i + 1;
                if *b == b'\n' {
                    // Found end-of-line. Strip trailing \r if present.
                    if let Some(prev) = buf.last() {
                        if *prev == b'\r' {
                            buf.pop();
                        }
                    }
                    self.inner.consume(consumed);
                    self.bytes_read += consumed as u64;
                    total += consumed;
                    if total > self.max_line {
                        return Err(Error::SmtpProto("line too long".into()));
                    }
                    // SMTP commands are 7-bit ASCII in practice; we
                    // accept UTF-8 too. Lossy conversion is fine — we
                    // only use this for command parsing.
                    return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
                }
                buf.push(*b);
                if buf.len() > self.max_line {
                    self.inner.consume(consumed);
                    self.bytes_read += consumed as u64;
                    return Err(Error::SmtpProto("line too long".into()));
                }
            }
            self.inner.consume(consumed);
            self.bytes_read += consumed as u64;
            total += consumed;
            if total > self.max_line {
                return Err(Error::SmtpProto("line too long".into()));
            }
        }
    }

    /// Borrow the underlying buffered reader (used by the DATA path).
    pub fn as_buf_mut(&mut self) -> &mut BufReader<R> {
        &mut self.inner
    }
}

/// Read exactly `n` bytes from the inner reader (used for tests/diagnostics).
pub async fn read_exact_n<R: AsyncRead + Unpin>(r: &mut R, n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn reads_a_couple_lines() {
        let input: &[u8] = b"HELO world\r\nMAIL FROM:<a@b>\r\n";
        let mut r = LineReader::new(BufReader::new(input), 200);
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("HELO world"));
        assert_eq!(
            r.next_line().await.unwrap().as_deref(),
            Some("MAIL FROM:<a@b>")
        );
        assert_eq!(r.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn line_too_long() {
        let input: Vec<u8> = vec![b'a'; 300];
        let mut r = LineReader::new(BufReader::new(&input[..]), 100);
        assert!(r.next_line().await.is_err());
    }
}
