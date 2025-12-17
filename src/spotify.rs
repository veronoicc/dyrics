//! Spotify client and synchronization logic.

use std::{sync::Arc, time::Duration};

use reqwest::Client;
use rspotify::{
    clients::OAuthClient as _,
    model::{AdditionalType, FullTrack, PlayableItem},
    prelude::BaseClient as _,
    scopes, AuthCodeSpotify, Config as SpotifyClientConfig, Credentials, OAuth,
};
use tokio::sync::RwLock;

use crate::{
    config::SpotifyConfig,
    error::{DyricsError, Result},
    lyrics::Lyrics,
};

/// Shared state for the current playback.
pub type PlaybackState = Arc<RwLock<Option<CurrentPlayback>>>;

/// Current playback information.
#[derive(Debug, Clone)]
pub struct CurrentPlayback {
    /// Currently playing track.
    pub track: FullTrack,
    /// Lyrics for the current track (if available).
    pub lyrics: Option<Lyrics>,
    /// Current playback position.
    pub position: Duration,
}

/// Create and authenticate a Spotify client.
pub async fn create_client(config: &SpotifyConfig) -> Result<AuthCodeSpotify> {
    let mut spotify = AuthCodeSpotify::with_config(
        Credentials::new(&config.client_id, &config.client_secret),
        OAuth {
            redirect_uri: config.redirect_uri.clone(),
            scopes: scopes!("user-read-currently-playing"),
            ..Default::default()
        },
        SpotifyClientConfig {
            token_cached: true,
            ..Default::default()
        },
    );

    if let Some(code) = &config.code {
        let (state, code) = code
            .split_once(':')
            .ok_or_else(|| DyricsError::Auth("Invalid code format, expected 'state:code'".into()))?;
        spotify.oauth.state = state.to_string();
        spotify
            .request_token(code)
            .await
            .map_err(|e| DyricsError::Auth(format!("Failed to request token: {e}")))?;
        spotify
            .write_token_cache()
            .await
            .map_err(|e| DyricsError::Auth(format!("Failed to write token cache: {e}")))?;
    } else {
        let url = spotify
            .get_authorize_url(false)
            .map_err(|e| DyricsError::Auth(format!("Failed to get authorize URL: {e}")))?;
        spotify
            .prompt_for_token(&url)
            .await
            .map_err(|e| DyricsError::Auth(format!("Failed to prompt for token: {e}")))?;
    }

    Ok(spotify)
}

/// Periodically increment the playback position to keep it in sync.
pub async fn step_loop(state: PlaybackState) {
    const STEP_INTERVAL: Duration = Duration::from_millis(50);

    loop {
        if let Some(ref mut playback) = *state.write().await {
            playback.position += STEP_INTERVAL;
        }
        tokio::time::sleep(STEP_INTERVAL).await;
    }
}

/// Periodically sync with Spotify to get current playback and fetch lyrics.
pub async fn resync_loop(
    state: PlaybackState,
    spotify: AuthCodeSpotify,
    resync_interval: Duration,
) -> Result<()> {
    let http = Client::new();
    let mut last_track_id: Option<rspotify::model::TrackId<'static>> = None;

    loop {
        match sync_once(&state, &spotify, &http, &mut last_track_id).await {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Sync error: {e}");
            }
        }
        tokio::time::sleep(resync_interval).await;
    }
}

/// Perform a single sync with Spotify.
async fn sync_once(
    state: &PlaybackState,
    spotify: &AuthCodeSpotify,
    http: &Client,
    last_track_id: &mut Option<rspotify::model::TrackId<'static>>,
) -> Result<()> {
    let currently_playing = spotify
        .current_playing(None, None::<Vec<&AdditionalType>>)
        .await?;

    let Some(playing) = currently_playing else {
        *last_track_id = None;
        *state.write().await = None;
        return Ok(());
    };

    if !playing.is_playing {
        *last_track_id = None;
        *state.write().await = None;
        return Ok(());
    }

    let Some(item) = playing.item else {
        *last_track_id = None;
        *state.write().await = None;
        return Ok(());
    };

    let PlayableItem::Track(track) = item else {
        *last_track_id = None;
        *state.write().await = None;
        return Ok(());
    };

    let position = playing
        .progress
        .map(|p| Duration::from_millis(p.num_milliseconds().max(0) as u64))
        .unwrap_or_default();

    // Check if track changed
    if track.id.as_ref().map(|id| id.to_string()) != last_track_id.as_ref().map(|id| id.to_string())
    {
        *last_track_id = track.id.clone().map(|id| id.clone_static());

        let lyrics = if let Some(ref track_id) = track.id {
            fetch_lyrics(spotify, http, track_id).await.ok()
        } else {
            None
        };

        *state.write().await = Some(CurrentPlayback {
            track,
            lyrics,
            position,
        });
    } else {
        // Just update position
        if let Some(ref mut playback) = *state.write().await {
            playback.position = position;
        }
    }

    Ok(())
}

/// Fetch lyrics from the beautiful-lyrics API.
async fn fetch_lyrics(
    spotify: &AuthCodeSpotify,
    http: &Client,
    track_id: &rspotify::model::TrackId<'_>,
) -> Result<Lyrics> {
    let token = spotify.token.lock().await.unwrap();
    let access_token = token
        .as_ref()
        .ok_or_else(|| DyricsError::Auth("No access token available".into()))?
        .access_token
        .clone();
    drop(token);

    let track_id_str = track_id.to_string().replace("spotify:track:", "");
    let url = format!("https://beautiful-lyrics.socalifornian.live/lyrics/{track_id_str}");

    let response = http.get(&url).bearer_auth(&access_token).send().await?;

    let lyrics: Lyrics = response
        .json()
        .await
        .map_err(|e| DyricsError::Lyrics(format!("Failed to parse lyrics: {e}")))?;

    Ok(lyrics)
}
