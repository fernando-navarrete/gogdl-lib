use std::{io::Seek, path::PathBuf, pin::Pin, task::{Context, Poll}};

use md5::{Digest as Md5DigestTrait, Md5};
use sha2::Sha256;
use std::io::Read;
use tokio::io::{self, AsyncWrite};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlgorithm {
    // Sha 256 not used, but available
    #[allow(dead_code)]
    Sha256,
    Md5,
}

pub async fn compute_chunk_checksum(
    path: PathBuf,
    offset: u64,
    size: u64,
    algo: ChecksumAlgorithm,
) -> io::Result<String> {
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        file.seek(std::io::SeekFrom::Start(offset))?;

        let mut buf = [0u8; 1024 * 1024];
        let mut remaining = size;

        let mut read_range = |update: &mut dyn FnMut(&[u8])| -> io::Result<()> {
            while remaining > 0 {
                let want = std::cmp::min(buf.len() as u64, remaining) as usize;
                let n = file.read(&mut buf[..want])?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "expected {size} bytes at offset {offset}, file ended {remaining} bytes short"
                        ),
                    ));
                }
                update(&buf[..n]);
                remaining -= n as u64;
            }
            Ok(())
        };

        let hex_digest = match algo {
            ChecksumAlgorithm::Sha256 => {
                let mut hasher = Sha256::new();
                read_range(&mut |chunk| hasher.update(chunk))?;
                hex::encode(hasher.finalize())
            }
            ChecksumAlgorithm::Md5 => {
                let mut hasher = Md5::new();
                read_range(&mut |chunk| hasher.update(chunk))?;
                hex::encode(hasher.finalize())
            }
        };

        Ok::<String, io::Error>(hex_digest)
    })
    .await
    .map_err(|join_err| io::Error::new(io::ErrorKind::Other, join_err.to_string()))?
}

pub struct HashingWriter<W: AsyncWrite + Unpin> {
    inner: W,
    hasher: Md5,
}

impl<W: AsyncWrite + Unpin> HashingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Md5::new(),
        }
    }

    pub fn into_parts(self) -> (W, String) {
        (self.inner, hex::encode(self.hasher.finalize()))
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for HashingWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_write(cx, buf) {
            Poll::Ready(Ok(n)) => {
                this.hasher.update(&buf[..n]);
                Poll::Ready(Ok(n))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}
