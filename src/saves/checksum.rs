use md5::{Digest, Md5};

/// The lowercase hex MD5 of `data`, in the form GOG uses for a stored save's
/// `ETag`.
pub fn md5_hex(data: &[u8]) -> String {
    hex::encode(Md5::digest(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_hex_matches_known_vector() {
        assert_eq!(md5_hex(b"hello world"), "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }

    #[test]
    fn md5_hex_differs_for_different_input() {
        assert_ne!(md5_hex(b"hello world"), md5_hex(b"hello worle"));
    }
}
