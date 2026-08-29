pub enum FileSizeVerificationEvent {
    FileWithNoChunks(String, u64),
    CouldNotResolvePath(String, u64),
    FileNotFound(String, u64),
    FileSizeVerificationFailed(String, u64),
    FileSizeMismatch(String, u64),
    FileSizeVerificationSuccess(String, u64),
}
