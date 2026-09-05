pub enum DownloadEvent {
    Preparing,
    Prepared,
    Downloading,
    /// Used to indicate progress (bytes downloaded)
    Progress(usize),
    /// Used to indicate progress regression (a chunk has failed and has to be retried)
    ProgressRegression(usize),
}
