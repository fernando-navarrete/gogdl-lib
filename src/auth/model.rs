use serde::{Deserialize, Serialize};

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
