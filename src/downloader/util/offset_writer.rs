use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncSeekExt, AsyncWrite};

pub struct OffsetWriter {
    file: tokio::fs::File,
    pos: u64,       // absolute file offset of the next byte to write
    remaining: u64, // bytes still allowed inside this unit's slot
}

impl OffsetWriter {
    pub async fn new(file: std::fs::File, offset: u64, size: u64) -> io::Result<Self> {
        let mut file = tokio::fs::File::from_std(file);
        file.seek(io::SeekFrom::Start(offset)).await?;
        Ok(Self {
            file,
            pos: offset,
            remaining: size,
        })
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
    }
}

impl AsyncWrite for OffsetWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if this.remaining == 0 {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "chunk decompressed past its declared size (at file offset {})",
                    this.pos
                ),
            )));
        }

        let want = std::cmp::min(buf.len() as u64, this.remaining) as usize;
        let n = match Pin::new(&mut this.file).poll_write(cx, &buf[..want]) {
            Poll::Ready(Ok(n)) => n,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        };
        if n == 0 {
            return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
        }
        this.pos += n as u64;
        this.remaining -= n as u64;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().file).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().file).poll_shutdown(cx)
    }
}
