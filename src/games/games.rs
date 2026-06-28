use std::sync::Arc;

use tokio::sync::Mutex;

use crate::auth::AuthManager;
use crate::client::HttpClient;
use crate::games::error::GamesError;
use crate::games::owned_games::OwnedGames;

pub struct GamesManager {
    pub inner: Arc<Mutex<GamesManagerInner>>,
    pub client: HttpClient,
}

pub struct GamesManagerInner {
    pub owned_games: OwnedGames,
    pub auth: AuthManager,
}

impl GamesManager {
    pub fn new(client: HttpClient, auth: AuthManager) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GamesManagerInner {
                owned_games: OwnedGames::default(),
                auth,
            })),
            client,
        }
    }
    pub async fn get_owned_games(&self) -> Result<OwnedGames, GamesError> {
        let auth = {
            let lock = self.inner.lock().await;
            if let None = lock.auth.get_auth().await {
                return Err(GamesError::NotAuthenticated);
            }
            lock.auth.get_auth().await.unwrap()
        };
        let url = "https://embed.gog.com/user/data/games";

        let owned_games: OwnedGames = match self
            .client
            .get_json_with_auth::<OwnedGames>(url, &auth.access_token)
            .await
            .map_err(GamesError::from)
        {
            Ok(owned_games) => owned_games,
            Err(GamesError::Unauthorized) => {
                // Token refresh logic
                let lock = self.inner.lock().await;
                if let Err(_err) = lock.auth.refresh_auth().await {
                    return Err(GamesError::Unauthorized);
                }
                let auth = lock.auth.get_auth().await.unwrap();
                match self
                    .client
                    .get_json_with_auth::<OwnedGames>(url, &auth.access_token)
                    .await
                    .map_err(GamesError::from)
                {
                    Ok(owned_games) => owned_games,
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
            Err(err) => {
                return Err(err);
            }
        };
        Ok(owned_games)
    }
}
