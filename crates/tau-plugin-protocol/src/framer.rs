//! Length-prefixed MessagePack frame reader and writer.
//!
//! Each frame on the wire is:
//!
//! ```text
//! +--------+--------+--------+--------+================+
//! |  big-endian u32 length (excl PFX) | MessagePack    |
//! +--------+--------+--------+--------+ message body  |
//!                                     +================+
//! ```

use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::ProtocolError;

const PREFIX_LEN: usize = 4;

/// Tunables for the framer. Default `max_message_size = 64 MiB`.
///
/// # Example
///
/// ```
/// use tau_plugin_protocol::FramerOptions;
/// let opts = FramerOptions::default();
/// assert_eq!(opts.max_message_size, 64 * 1024 * 1024);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct FramerOptions {
    /// Reject frames whose length-prefix exceeds this many bytes.
    pub max_message_size: usize,
}

impl Default for FramerOptions {
    fn default() -> Self {
        Self {
            max_message_size: 64 * 1024 * 1024,
        }
    }
}

/// Async reader for length-prefixed MessagePack frames.
pub struct FramedReader<R> {
    inner: R,
    options: FramerOptions,
    buf: BytesMut,
}

impl<R> FramedReader<R>
where
    R: AsyncRead + Unpin,
{
    /// Construct a new reader.
    pub fn new(inner: R, options: FramerOptions) -> Self {
        Self {
            inner,
            options,
            buf: BytesMut::with_capacity(8192),
        }
    }

    /// Read the next frame body from the underlying transport. Returns
    /// `Ok(None)` on clean EOF (zero bytes when no frame is in
    /// progress).
    pub async fn next_frame(&mut self) -> Result<Option<Vec<u8>>, ProtocolError> {
        let mut prefix = [0u8; PREFIX_LEN];
        match self.inner.read_exact(&mut prefix).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(ProtocolError::Io(e)),
        }
        let len = u32::from_be_bytes(prefix) as usize;
        if len > self.options.max_message_size {
            return Err(ProtocolError::FrameTooLarge {
                len,
                max: self.options.max_message_size,
            });
        }
        self.buf.clear();
        self.buf.resize(len, 0);
        if let Err(e) = self.inner.read_exact(&mut self.buf[..]).await {
            return Err(if e.kind() == std::io::ErrorKind::UnexpectedEof {
                ProtocolError::FrameTruncated { expected: len }
            } else {
                ProtocolError::Io(e)
            });
        }
        Ok(Some(self.buf[..len].to_vec()))
    }
}

/// Async writer for length-prefixed MessagePack frames.
pub struct FramedWriter<W> {
    inner: W,
}

impl<W> FramedWriter<W>
where
    W: AsyncWrite + Unpin,
{
    /// Construct a new writer.
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Write a frame body. The length prefix is computed automatically.
    pub async fn write_frame(&mut self, body: &[u8]) -> Result<(), ProtocolError> {
        let len = body.len() as u32;
        self.inner.write_all(&len.to_be_bytes()).await?;
        self.inner.write_all(body).await?;
        self.inner.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn round_trip_small_frame() {
        let (a, b) = duplex(8192);
        let mut writer = FramedWriter::new(a);
        let mut reader = FramedReader::new(b, FramerOptions::default());
        writer.write_frame(b"hello").await.unwrap();
        let frame = reader.next_frame().await.unwrap().unwrap();
        assert_eq!(frame, b"hello");
    }

    #[tokio::test]
    async fn read_returns_none_on_clean_eof() {
        let (a, b) = duplex(8);
        drop(a); // close write side
        let mut reader = FramedReader::new(b, FramerOptions::default());
        let result = reader.next_frame().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn frame_too_large_rejected() {
        let (a, b) = duplex(64);
        let opts = FramerOptions {
            max_message_size: 4,
        };
        let mut writer = FramedWriter::new(a);
        let mut reader = FramedReader::new(b, opts);
        writer.write_frame(b"hello").await.unwrap();
        let err = reader.next_frame().await.unwrap_err();
        let ProtocolError::FrameTooLarge { len, max } = err else {
            panic!("expected FrameTooLarge")
        };
        assert_eq!(len, 5);
        assert_eq!(max, 4);
    }

    #[tokio::test]
    async fn truncated_body_returns_frame_truncated() {
        let (mut a, b) = duplex(64);
        // Write the prefix claiming 100 bytes, then close before
        // sending body.
        let prefix = (100u32).to_be_bytes();
        a.write_all(&prefix).await.unwrap();
        drop(a);
        let mut reader = FramedReader::new(b, FramerOptions::default());
        let err = reader.next_frame().await.unwrap_err();
        let ProtocolError::FrameTruncated { expected } = err else {
            panic!("expected FrameTruncated")
        };
        assert_eq!(expected, 100);
    }
}
