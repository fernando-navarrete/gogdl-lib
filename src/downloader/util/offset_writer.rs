use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::FileExt;

pub struct OffsetWriter {
    file: File,
    pos: u64,       // absolute file offset of the next byte to write
    remaining: u64, // bytes still allowed inside this unit's slot
}

impl OffsetWriter {
    pub fn new(file: File, offset: u64, size: u64) -> Self {
        Self {
            file,
            pos: offset,
            remaining: size,
        }
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
    }
}

impl Write for OffsetWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "chunk decompressed past its declared size (at file offset {})",
                    self.pos
                ),
            ));
        }

        let want = std::cmp::min(buf.len() as u64, self.remaining) as usize;
        let n = self.file.write_at(&buf[..want], self.pos)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        self.pos += n as u64;
        self.remaining -= n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
