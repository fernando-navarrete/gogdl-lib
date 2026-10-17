use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    client::{
        HttpClient,
        auth::{Auth, AuthError, Grant, TokenObserver},
    },
    constants::{CLIENT_ID, CLIENT_SECRET, LOGIN_URL, REDIRECT_URI, TOKEN_URL},
};

#[derive(Clone)]
pub struct AuthManager {
    inner: Arc<Mutex<AuthManagerInner>>,
    refresh_lock: Arc<Mutex<()>>,
    token_url: String,
}

pub struct AuthManagerInner {
    tokens: Option<Auth>,
    token_observer: Option<Arc<dyn TokenObserver>>,
}

impl AuthManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(AuthManagerInner {
                tokens: None,
                token_observer: None,
            })),
            refresh_lock: Arc::new(Mutex::new(())),
            token_url: TOKEN_URL.to_string(),
        }
    }
    #[cfg(test)]
    pub fn with_token_url(url: String) -> Self {
        Self {
            token_url: url,
            ..Self::new()
        }
    }
    pub fn get_login_url(&self) -> &str {
        LOGIN_URL
    }
    pub async fn set_token_observer(&self, observer: Arc<dyn TokenObserver>) {
        self.inner.lock().await.token_observer = Some(observer);
    }
    pub async fn remove_token_observer(&self) {
        self.inner.lock().await.token_observer = None;
    }
    pub async fn login_with_code(
        &self,
        code: &str,
        client: &HttpClient,
    ) -> Result<String, AuthError> {
        let auth = Self::exchange(
            client,
            &self.token_url,
            Grant::AuthorizationCode,
            &[
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
                ("code", code),
                ("redirect_uri", REDIRECT_URI),
            ],
        )
        .await?;
        let json_str = auth.to_string()?;
        self.inner.lock().await.tokens = Some(auth);
        Ok(json_str)
    }
    pub async fn restore_from_string(&self, json_str: &str) -> Result<(), AuthError> {
        let tokens = Auth::from_string(json_str)?;
        self.inner.lock().await.tokens = Some(tokens);
        Ok(())
    }
    pub async fn refresh_auth(&self, client: &HttpClient) -> Result<(), AuthError> {
        // Snapshot before queueing on refresh_lock, so we can tell whether another
        // waiter refreshed while we waited.
        let stale_token = {
            let inner = self.inner.lock().await;
            match inner.tokens.as_ref() {
                Some(tokens) => tokens.access_token.clone(),
                None => return Err(AuthError::NotAuthenticated),
            }
        };

        // Lock to prevent concurrent refresh attempts
        let _lock = self.refresh_lock.lock().await;
        let tokens = {
            let tokens = self.inner.lock().await.tokens.clone();
            if tokens.is_none() {
                return Err(AuthError::NotAuthenticated);
            }
            tokens.unwrap()
        };

        // Another waiter already refreshed while we were queued — reuse its result.
        if tokens.access_token != stale_token {
            return Ok(());
        }

        let auth = Self::exchange(
            client,
            &self.token_url,
            Grant::RefreshToken,
            &[
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
                ("refresh_token", &tokens.refresh_token),
            ],
        )
        .await?;

        {
            let mut inner = self.inner.lock().await;
            inner.tokens = Some(auth.clone());
            let observer = inner
                .token_observer
                .as_ref()
                .map(|observer| observer.clone());
            drop(inner);
            if let Some(observer) = observer {
                observer.on_token_refreshed(auth.clone());
            }
        }
        Ok(())
    }
    /// POSTs a token grant to `url` and stamps `valid_until`. The grant goes
    /// in a form body and the transport error has its URL stripped, so
    /// neither the code nor the refresh token can reach an error string.
    /// A failure is classified by `grant` (see [`AuthError::from_token_failure`]).
    async fn exchange(
        client: &HttpClient,
        url: &str,
        grant: Grant,
        form: &[(&str, &str)],
    ) -> Result<Auth, AuthError> {
        let mut form = form.to_vec();
        form.push(("grant_type", grant.as_str()));
        let mut auth: Auth = client
            .post_token(url, &form)
            .await
            .map_err(|failure| AuthError::from_token_failure(grant, failure))?;
        auth.valid_until = Some(auth.expires_in as i64 + chrono::Utc::now().timestamp());
        Ok(auth)
    }
    pub async fn get_auth(&self) -> Result<Auth, AuthError> {
        let auth = {
            let lock = self.inner.lock().await;
            if let Some(auth) = lock.tokens.as_ref() {
                auth.clone()
            } else {
                return Err(AuthError::NotAuthenticated);
            }
        };
        if auth.is_valid() {
            Ok(auth)
        } else {
            Err(AuthError::TokenExpired)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ChunkServer, Reply};

    const REFRESH: &str = "REFRESH-SENTINEL";
    const CODE: &str = "CODE-SENTINEL";
    const LOGIN_FIXTURE: &str =
        include_str!("../../../tests/fixtures/token_error_bad_login_code.json");
    const REFRESH_FIXTURE: &str =
        include_str!("../../../tests/fixtures/token_error_refresh_garbage.json");

    fn no_secrets(err: &AuthError) {
        for text in [err.to_string(), format!("{err:?}")] {
            assert!(!text.is_empty());
            for secret in [REFRESH, CODE, CLIENT_SECRET] {
                assert!(!text.contains(secret), "{secret} leaked: {text}");
            }
        }
    }

    fn form(grant: Grant) -> Vec<(&'static str, &'static str)> {
        vec![
            ("client_secret", CLIENT_SECRET),
            match grant {
                Grant::AuthorizationCode => ("code", CODE),
                Grant::RefreshToken => ("refresh_token", REFRESH),
            },
        ]
    }

    async fn exchange_against(reply: Reply, grant: Grant) -> AuthError {
        let server = ChunkServer::start().await;
        server.script("/token", vec![reply]);
        let url = format!("{}/token", server.base_url());
        let err = AuthManager::exchange(
            &HttpClient::new_with_client(reqwest::Client::new()),
            &url,
            grant,
            &form(grant),
        )
        .await
        .err()
        .unwrap();
        no_secrets(&err);
        err
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply::StatusBody(status, body.as_bytes().to_vec())
    }

    // Captured from the real endpoint (v1.4.0 plan, step 1): every dead grant is
    // `400 invalid_grant`, and the grant decides the kind.
    #[tokio::test]
    async fn token_error_fixtures_map_to_their_kinds() {
        let err = exchange_against(reply(400, LOGIN_FIXTURE), Grant::AuthorizationCode).await;
        let AuthError::InvalidLoginCode { description } = &err else {
            panic!("expected InvalidLoginCode, got {err:?}");
        };
        assert_eq!(
            description,
            "Code doesn't exist or is invalid for the client"
        );

        let err = exchange_against(reply(400, REFRESH_FIXTURE), Grant::RefreshToken).await;
        let AuthError::SessionRevoked { description } = &err else {
            panic!("expected SessionRevoked, got {err:?}");
        };
        assert!(
            description.starts_with("Invalid refresh token"),
            "{description}"
        );
    }

    #[tokio::test]
    async fn the_grant_picks_the_kind_not_the_body() {
        let err = exchange_against(reply(400, REFRESH_FIXTURE), Grant::AuthorizationCode).await;
        assert!(matches!(err, AuthError::InvalidLoginCode { .. }), "{err:?}");
        let err = exchange_against(reply(400, LOGIN_FIXTURE), Grant::RefreshToken).await;
        assert!(matches!(err, AuthError::SessionRevoked { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn other_failures_fall_back_to_rejected() {
        let err = exchange_against(Reply::Status(503), Grant::RefreshToken).await;
        assert!(
            matches!(&err, AuthError::Rejected { status, body } if status.as_u16() == 503 && body.is_empty()),
            "{err:?}"
        );
        for body in [r#"{"error":"server_error"}"#, "<html>nope</html>"] {
            let err = exchange_against(reply(400, body), Grant::RefreshToken).await;
            assert!(
                matches!(&err, AuthError::Rejected { status, body: got } if status.as_u16() == 400 && got == body),
                "{err:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_transport_failure_is_a_network_error() {
        for grant in [Grant::AuthorizationCode, Grant::RefreshToken] {
            let err = exchange_against(Reply::Close, grant).await;
            assert!(matches!(err, AuthError::NetworkError(_)), "{err:?}");
        }
    }

    #[tokio::test]
    async fn an_undecodable_success_is_an_invalid_response() {
        let err = exchange_against(Reply::Body(b"{}".to_vec()), Grant::RefreshToken).await;
        assert!(matches!(err, AuthError::InvalidResponse(_)), "{err:?}");
    }
}
