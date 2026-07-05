use std::path::PathBuf;

use md5::{Digest as Md5DigestTrait, Md5};
use sha2::Sha256;
use std::io::Read;
use tokio::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlgorithm {
    Sha256,
    Md5,
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
