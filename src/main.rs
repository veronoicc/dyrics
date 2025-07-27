use std::{
    sync::Arc,
    time::Duration,
};

use figment::{
    providers::{Env, Format as _, Toml},
    Figment,
};
use once_cell::sync::Lazy;
use reqwest::Client;
use rspotify::{
    clients::OAuthClient as _, model::{AdditionalType, FullTrack, PlayableItem}, prelude::BaseClient as _, scopes, AuthCodeSpotify, Config as SpotifyClientConfig, Credentials, OAuth
};
use serde::Deserialize;
use serde_json::json;
use serde_with::serde_as;
use serde_with::DurationSeconds;
use tokio::sync::RwLock;

static DISCORD_REQWEST: Lazy<reqwest::Client> = Lazy::new(|| reqwest::Client::new());

#[derive(Debug, Clone, Deserialize)]
struct Config {
    discord: DiscordConfig,
    spotify: SpotifyConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct DiscordConfig {
    token: String,
}

fn default_redirect_uri() -> String {
    "https://127.0.0.1".to_string()
}

fn default_resync_interval() -> Duration {
    Duration::from_secs_f32(2.5)
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
struct SpotifyConfig {
    client_id: String,
    client_secret: String,
    #[serde(default = "default_redirect_uri")]
    redirect_uri: String,
    #[serde_as(as = "DurationSeconds<f64>")]
    #[serde(default = "default_resync_interval")]
    resync_interval: Duration,
    #[serde(skip_serializing_if = "Option::is_none")]
    redirected_url: Option<String>,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let config = Figment::new()
        .merge(Toml::file("config.toml"))
        .merge(Env::prefixed("CONFIG_").split("_"))
        .extract::<Config>()?;

    let spotify = AuthCodeSpotify::with_config(
        Credentials::new(&config.spotify.client_id, &config.spotify.client_secret),
        OAuth {
            redirect_uri: config.spotify.redirect_uri.clone(),
            scopes: scopes!("user-read-currently-playing"),
            ..Default::default()
        },
        SpotifyClientConfig {
            token_cached: true,
            ..Default::default()
        }
    );

    // Handle authentication
    if let Some(redirected_url) = &config.spotify.redirected_url {
        // Use provided redirect URL to extract code and request token
        if let Some(code) = spotify.parse_response_code(redirected_url) {
            spotify.request_token(&code).await.unwrap();
            spotify.write_token_cache().await.unwrap();
        } else {
            return Err(eyre::eyre!("Failed to parse authorization code from redirected URL"));
        }
    } else {
        // Prompt for token as before
        spotify
            .prompt_for_token(&spotify.get_authorize_url(false).unwrap())
            .await
            .unwrap();
    }

    let current_lyrics = Arc::new(RwLock::new(Option::None));

    tokio::spawn(step_loop(current_lyrics.clone()));

    tokio::try_join!(
        resync_loop(
            current_lyrics.clone(),
            spotify,
            config.spotify.resync_interval
        ),
        status_loop(current_lyrics.clone(), &config.discord.token),
    )?;

    Ok(())
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Lyrics {
    #[serde_as(as = "DurationSeconds<f64>")]
    start_time: Duration,
    #[serde_as(as = "DurationSeconds<f64>")]
    end_time: Duration,
    #[serde(flatten)]
    content: LyricsContent,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase", tag = "Type", content = "Content")]
enum LyricsContent {
    Syllable(Vec<SyllableLyricsLine>),
    Line(Vec<LineLyricsLine>),
    //Static(), TODO: Fix, we need not "content" but "lines" for this
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SyllableLyricsLine {
    r#type: String,
    opposite_aligned: bool,
    lead: SyllableLyricsLead,
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SyllableLyricsLead {
    syllables: Vec<SyllableLyricsSyllable>,
    #[serde_as(as = "DurationSeconds<f64>")]
    start_time: Duration,
    #[serde_as(as = "DurationSeconds<f64>")]
    end_time: Duration,
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SyllableLyricsSyllable {
    text: String,
    is_part_of_word: bool,
    #[serde_as(as = "DurationSeconds<f64>")]
    start_time: Duration,
    #[serde_as(as = "DurationSeconds<f64>")]
    end_time: Duration,
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LineLyricsLine {
    r#type: String,
    opposite_aligned: bool,
    text: String,
    #[serde_as(as = "DurationSeconds<f64>")]
    start_time: Duration,
    #[serde_as(as = "DurationSeconds<f64>")]
    end_time: Duration,
}

async fn step_loop(current_lyrics: Arc<RwLock<Option<(Option<Lyrics>, FullTrack, Duration)>>>) {
    loop {
        if let Some(ref mut tuple) = *current_lyrics.write().await {
            tuple.2 = tuple.2 + Duration::from_millis(50);
        }
        tokio::time::sleep(Duration::from_millis(50)).await
    }
}

async fn resync_loop(
    current_lyrics: Arc<RwLock<Option<(Option<Lyrics>, FullTrack, Duration)>>>,
    spotify: AuthCodeSpotify,
    resync_interval: Duration,
) -> eyre::Result<()> {
    let mut last_playing = None;
    let reqwest = Client::new();

    loop {
        if let Some(currently_playing) = spotify
            .current_playing(None, None::<Vec<&AdditionalType>>)
            .await?
        {
            if !currently_playing.is_playing || currently_playing.item.is_none() {
                last_playing = None;
                *current_lyrics.write().await = None;
                tokio::time::sleep(resync_interval).await;
                continue;
            }

            if let PlayableItem::Track(track) = currently_playing.item.unwrap() {
                if track.id != last_playing {
                    last_playing = track.id.clone();

                    if let Some(ref track_id) = track.id {
                        let response = reqwest
                            .get(format!(
                                "https://beautiful-lyrics.socalifornian.live/lyrics/{}",
                                track_id.to_string().replace("spotify:track:", "")
                            ))
                            .bearer_auth(
                                spotify
                                    .token
                                    .lock()
                                    .await
                                    .unwrap()
                                    .clone()
                                    .unwrap()
                                    .access_token,
                            )
                            .send()
                            .await?;
                        let lyrics = response.json().await;

                        if let Ok(lyrics) = lyrics {
                            *current_lyrics.write().await = Some((
                                Some(lyrics),
                                track.clone(),
                                Duration::from_millis(
                                    currently_playing.progress.unwrap().num_milliseconds() as u64,
                                ),
                            ))
                        } else {
                            *current_lyrics.write().await = Some((
                                None,
                                track.clone(),
                                Duration::from_millis(
                                    currently_playing.progress.unwrap().num_milliseconds() as u64,
                                ),
                            ))
                        }
                    }
                } else {
                    // only update the timestamp
                    if let Some(ref mut tuple) = *current_lyrics.write().await {
                        tuple.2 = Duration::from_millis(
                            currently_playing.progress.unwrap().num_milliseconds() as u64,
                        )
                    }
                }
            } else {
                last_playing = None;
                *current_lyrics.write().await = None;
                tokio::time::sleep(resync_interval).await;
                continue;
            }
        }

        tokio::time::sleep(resync_interval).await;
    }
}

async fn status_loop(
    current_lyrics: Arc<RwLock<Option<(Option<Lyrics>, FullTrack, Duration)>>>,
    token: &str,
) -> eyre::Result<()> {
    let mut last_text = None;

    loop {
        if let Some((current_lyrics, track, current_time)) = current_lyrics.read().await.clone() {
            let text = if let Some(lyrics) = current_lyrics {
                match lyrics.content {
                    LyricsContent::Syllable(syllables) => {
                        let syllable = syllable_find_nearest(&syllables, current_time);

                        if let Some(syllable) = syllable {
                            syllable
                                .lead
                                .syllables
                                .iter()
                                .map(|val| val.text.to_string())
                                .collect::<Vec<_>>()
                                .join(" ")
                        } else {
                            "".to_string()
                        }
                    }
                    LyricsContent::Line(lines) => {
                        let line = line_find_nearest(&lines, current_time);

                        line.map(|val| val.text.to_string())
                            .unwrap_or("".to_string())
                    }
                }
            } else {
                format!(
                    "{} - {}",
                    track.name,
                    track
                        .artists
                        .iter()
                        .map(|val| val.name.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            };

            if let Some(ref last_text_loc) = last_text {
                if last_text_loc != &text {
                    set_discord_status(&text, "🎶", token).await?;
                    println!("New text is: {}", text);
                    last_text = Some(text);
                }
            } else {
                set_discord_status(&text, "🎶", token).await?;
                println!("New text is: {}", text);
                last_text = Some(text);
            }
        } else {
            if last_text.is_some() {
                set_discord_status("", "", token).await?;
                last_text = None;
            }
        }
        tokio::time::sleep(Duration::from_micros(300)).await;
    }
}

fn syllable_contains_duration(item: &SyllableLyricsLead, duration: Duration) -> bool {
    item.start_time <= duration && duration <= item.end_time
}

fn syllable_distance_to(item: &SyllableLyricsLead, duration: Duration) -> Duration {
    if duration < item.start_time {
        item.start_time - duration
    } else if duration > item.end_time {
        duration - item.end_time
    } else {
        Duration::from_secs(0)
    }
}

fn syllable_find_nearest<'a>(
    items: &'a [SyllableLyricsLine],
    target: Duration,
) -> Option<&'a SyllableLyricsLine> {
    items.iter().min_by_key(|item| {
        if syllable_contains_duration(&item.lead, target) {
            Duration::from_secs(0)
        } else {
            syllable_distance_to(&item.lead, target)
        }
    })
}

fn line_contains_duration(line: &LineLyricsLine, duration: Duration) -> bool {
    line.start_time <= duration && duration <= line.end_time
}

fn line_distance_to(line: &LineLyricsLine, duration: Duration) -> Duration {
    if duration < line.start_time {
        line.start_time - duration
    } else if duration > line.end_time {
        duration - line.end_time
    } else {
        Duration::from_secs(0)
    }
}

fn line_find_nearest<'a>(
    lines: &'a [LineLyricsLine],
    target: Duration,
) -> Option<&'a LineLyricsLine> {
    lines.iter().min_by_key(|line| {
        if line_contains_duration(line, target) {
            Duration::from_secs(0)
        } else {
            line_distance_to(line, target)
        }
    })
}

async fn set_discord_status(text: &str, emoji: &str, token: &str) -> eyre::Result<()> {
    DISCORD_REQWEST
        .patch("https://discord.com/api/v6/users/@me/settings")
        .header("authorization", token)
        .json(&json!({
            "custom_status": {
                "text": text,
                "emoji_name": emoji
            }
        }))
        .send()
        .await?;

    Ok(())
}