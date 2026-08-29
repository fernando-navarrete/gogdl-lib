pub enum FileAllocationEvent {
    FileWithNoChunks(String, u64),
    CouldNotResolvePath(String, u64),
    FileAllocationSuccess(String, u64),
}
