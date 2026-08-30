pub trait TokenObserver: Send + Sync + 'static {
    fn on_token_refreshed(&self, token: &str);
}
