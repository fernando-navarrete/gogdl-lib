use reqwest::Client;
use serde::de::DeserializeOwned;

use crate::client::error::ClientError;

#[derive(Clone)]
pub struct HttpClient {
    client: Client,
}

impl HttpClient {
    pub fn new_with_client(client: Client) -> Self {
        Self { client }
    }
    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let request = self.client.get(url);
        let response = request.send().await?;

        if !response.status().is_success() {
            let response_status = response.status();
            let response_text = response.text().await?;
            return Err(ClientError::Http {
                status: response_status,
                body: response_text,
            });
        }
        let result: T = response.json::<T>().await?;
        Ok(result)
    }
}
