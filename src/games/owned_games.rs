use std::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::games::game_details::GameDetails;

#[derive(Serialize, Deserialize, Clone)]
pub struct OwnedGames(Vec<GameDetails>);

impl OwnedGames {
    pub fn default() -> Self {
        Self(vec![])
    }
}
