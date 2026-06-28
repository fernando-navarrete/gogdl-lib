use crate::auth::AuthManager;
use crate::client::HttpClient;
use crate::gogdl::error::GogDlError;

pub struct GogDl {
    client: HttpClient,
    auth: AuthManager,
}

impl GogDl {
    pub fn new_from_client(client: reqwest::Client) -> Self {
        let http_client = HttpClient::new_with_client(client);
        let auth_manager = AuthManager::new(http_client.clone());
        Self {
            client: http_client,
            auth: auth_manager,
        }
    }
    pub fn get_login_url(&self) -> &str {
        self.auth.get_login_url()
    }
    pub async fn refresh_auth(&self) -> Result<(), GogDlError> {
        self.auth.refresh_auth().await?;
        Ok(())
    }
    pub async fn login_with_code(&self, code: &str) -> Result<(), GogDlError> {
        self.auth.login_with_code(code).await?;
        Ok(())
    }
}
