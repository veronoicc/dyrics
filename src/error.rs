//! Custom error types for Dyrics.

use thiserror::Error;

/// Main error type for the Dyrics application.
#[derive(Debug, Error)]
pub enum DyricsError {
    /// Configuration loading or parsing error.
    #[error("Configuration error: {0}")]
    Config(#[from] Box<figment::Error>),

    /// Spotify API client error.
    #[error("Spotify error: {0}")]
    Spotify(#[from] rspotify::ClientError),

    /// HTTP request error.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// Discord API error.
    #[error("Discord error: {0}")]
    Discord(String),

    /// Authentication error.
    #[error("Authentication error: {0}")]
    Auth(String),

    /// Lyrics parsing error.
    #[error("Lyrics error: {0}")]
    Lyrics(String),
}

/// Convenience type alias for Results using DyricsError.
pub type Result<T> = std::result::Result<T, DyricsError>;
