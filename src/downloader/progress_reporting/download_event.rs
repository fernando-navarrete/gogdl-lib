pub enum DownloadEvent {
    Preparing,
    Prepared,
    Downloading,
    Progress(usize),
}
