use serde::{Deserialize, Serialize};

use crate::auth::AuthError;

#[derive(Deserialize, Serialize, Clone)]
pub struct Auth {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i32,
    pub token_type: String,
    pub session_id: String,
    pub scope: Option<String>,
    pub user_id: String,
    #[serde(skip_deserializing)]
    pub valid_until: Option<i64>,
}

impl Auth {
    pub fn to_string(&self) -> Result<String, AuthError> {
        let json_str = serde_json::to_string(self)?;
        Ok(json_str)
    }
    pub fn from_string(json_str: &str) -> Result<Auth, AuthError> {
        let tokens: Auth = serde_json::from_str(json_str)?;
        Ok(tokens)
    }
}
