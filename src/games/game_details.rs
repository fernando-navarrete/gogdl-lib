use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GameDetails {
    pub title: String,
    #[serde(skip)]
    pub id: i32,
}
