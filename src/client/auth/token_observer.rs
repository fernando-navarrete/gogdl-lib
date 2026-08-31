use crate::client::auth::Auth;

pub trait TokenObserver: Send + Sync + 'static {
    fn on_token_refreshed(&self, auth: Auth);
}
