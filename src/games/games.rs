use crate::auth::AuthManager;
use crate::client::HttpClient;
use crate::games::owned_games::OwnedGames;

pub struct GamesManager {
    pub owned_games: OwnedGames,
    pub client: HttpClient,
    pub auth: AuthManager,
}

impl GamesManager {
    pub fn new(client: HttpClient, auth: AuthManager) -> Self {
        Self {
            owned_games: OwnedGames::default(),
            client,
            auth,
        }
    }
    pub async fn get_owned_games(&self) {}
}
