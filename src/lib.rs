//! Dyrics - Discord lyrics status updater for Spotify.
//!
//! Syncs your currently playing Spotify track's lyrics to your Discord status.

pub mod config;
pub mod discord;
pub mod error;
pub mod lyrics;
pub mod spotify;

pub use config::Config;
pub use error::{DyricsError, Result};
