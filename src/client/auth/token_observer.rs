use crate::client::auth::Auth;

/// Callback for observing internal auth-token refreshes. Register one with
/// [`GogDl::set_token_observer`](crate::GogDl::set_token_observer) to be
/// notified whenever a request triggers a token refresh — this is the only
/// way to learn that the refresh token rotated, so persist the new [`Auth`]
/// from here if you want the next run's
/// [`GogDl::restore_auth`](crate::GogDl::restore_auth) to still work.
pub trait TokenObserver: Send + Sync + 'static {
    /// Called with the freshly-refreshed auth state immediately after a
    /// successful token refresh. Runs synchronously on the task that
    /// triggered the refresh, without holding any internal lock.
    fn on_token_refreshed(&self, auth: Auth);
}
