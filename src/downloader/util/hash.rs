use std::{io::Seek, path::PathBuf};

use md5::{Digest as Md5DigestTrait, Md5};
use sha2::Sha256;
use std::io::Read;
use tokio::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlgorithm {
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

pub async fn compute_checksum(path: PathBuf, algo: ChecksumAlgorithm) -> io::Result<String> {
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        let mut buf = [0u8; 1024 * 1024];
        let hex_digest = match algo {
            ChecksumAlgorithm::Sha256 => {
                let mut hasher = Sha256::new();
                loop {
                    let n = file.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                }
                hex::encode(hasher.finalize())
            }
            ChecksumAlgorithm::Md5 => {
                let mut hasher = Md5::new();
                loop {
                    let n = file.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                }
                hex::encode(hasher.finalize())
            }
        };

        Ok::<String, io::Error>(hex_digest)
    })
    .await
    .map_err(|join_err| io::Error::new(io::ErrorKind::Other, join_err.to_string()))?
}
