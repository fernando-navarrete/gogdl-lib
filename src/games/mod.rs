mod error;
mod game_build;
mod game_details;
mod game_links;
mod game_screenshots;
mod game_summary;
mod games_manager;
mod owned_games;

pub use error::GamesError;
// `GameBuild` is already reachable structurally via `GameBuilds.items` (a
// `pub` field); re-exporting the type itself just lets other modules name it
// directly, which the `saves` module's tests do to build a `GameBuilds`
// fixture. Outside tests nothing needs the name, hence the allow.
#[allow(unused_imports)]
pub use game_build::GameBuild;
pub use game_build::GameBuilds;
pub use game_details::GameDetails;
pub use game_links::GameLinks;
pub use game_screenshots::GameScreenshots;
pub use game_summary::GameSummary;
pub use games_manager::GamesManager;
pub use owned_games::OwnedGames;
