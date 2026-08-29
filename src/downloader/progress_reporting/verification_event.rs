pub enum VerificationEvent {
    CouldNotResolvePath(String, u64),
    FileNotFound(String, u64),
    ChecksumMismatch(String, u64),
    Verified(String, u64),
}
