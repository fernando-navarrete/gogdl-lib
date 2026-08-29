pub enum VerificationEvent {
    CouldNotResolvePath(String),
    FileNotFound(String),
    ChecksumMismatch(String),
    Verified(String),
}
