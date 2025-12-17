//! Configuration types and loading for Dyrics.

use std::time::Duration;

use figment::{
    providers::{Env, Format as _, Toml},
    Figment,
};
use serde::Deserialize;
use serde_with::{serde_as, DurationSeconds};

use crate::error::Result;

/// Main application configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Discord-related configuration.
    pub discord: DiscordConfig,
    /// Spotify-related configuration.
    pub spotify: SpotifyConfig,
}

impl Config {
    /// Load configuration from config.toml and environment variables.
    pub fn load() -> Result<Self> {
        let config = Figment::new()
            .merge(Toml::file("config.toml"))
            .merge(Env::prefixed("CONFIG_").split("_"))
            .extract::<Config>()
            .map_err(Box::new)?;
        Ok(config)
    }
}

/// Discord configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordConfig {
    /// Discord user token for status updates.
    pub token: String,
}

/// Spotify configuration.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
pub struct SpotifyConfig {
    /// Spotify application client ID.
    pub client_id: String,
    /// Spotify application client secret.
    pub client_secret: String,
    /// OAuth redirect URI.
    #[serde(default = "default_redirect_uri")]
    pub redirect_uri: String,
    /// Interval between Spotify API syncs.
    #[serde_as(as = "DurationSeconds<f64>")]
    #[serde(default = "default_resync_interval")]
    pub resync_interval: Duration,
    /// Optional authorization code for initial setup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

fn default_redirect_uri() -> String {
    "https://127.0.0.1".to_string()
}

fn default_resync_interval() -> Duration {
    Duration::from_secs_f32(2.5)
}
