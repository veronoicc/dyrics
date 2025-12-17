//! Discord status updates with lookahead rate limiting.
//!
//! Implements a rate limiter that allows at most 3 status updates per 10 seconds.
//! Uses lookahead to proactively batch lines that would exceed the rate limit,
//! ensuring all lyrics are displayed at the correct time.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use reqwest::Client;
use serde_json::json;

use crate::{
    error::{DyricsError, Result},
    lyrics::TimedLine,
    spotify::PlaybackState,
};

/// Rate limit: maximum updates allowed within the window.
const RATE_LIMIT_MAX_UPDATES: usize = 3;
/// Rate limit: time window in seconds.
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(10);
/// Minimum interval between updates (window / max_updates).
const MIN_UPDATE_INTERVAL: Duration = Duration::from_millis(3334); // 10s / 3
/// Separator used when batching multiple lines together.
const BATCH_SEPARATOR: &str = ". ";
/// Smoothing factor for latency estimation (0.0 = no change, 1.0 = replace completely).
const LATENCY_SMOOTHING: f64 = 0.2;
/// Minimum latency clamp (ms).
const MIN_LATENCY_MS: u64 = 10;
/// Maximum latency clamp (ms).
const MAX_LATENCY_MS: u64 = 2000;

/// A scheduled status update.
#[derive(Debug, Clone)]
struct ScheduledUpdate {
    /// When this update should be displayed (song time, not wall time).
    display_time: Duration,
    /// The text to display (may be multiple lines batched).
    text: String,
}

/// Rate limiter with lookahead batching for Discord status updates.
#[derive(Debug)]
pub struct RateLimiter {
    /// HTTP client for Discord API.
    client: Client,
    /// Timestamps of recent updates within the rate limit window.
    timestamps: VecDeque<Instant>,
    /// Scheduled updates queue.
    schedule: VecDeque<ScheduledUpdate>,
    /// Estimated one-way latency to Discord (for lookahead).
    latency_estimate: Duration,
    /// Last sent text (to avoid duplicate sends).
    last_sent: Option<String>,
}

impl RateLimiter {
    /// Create a new rate limiter.
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            timestamps: VecDeque::with_capacity(RATE_LIMIT_MAX_UPDATES),
            schedule: VecDeque::new(),
            latency_estimate: Duration::ZERO,
            last_sent: None,
        }
    }

    /// Clean up old timestamps outside the rate limit window.
    fn cleanup_old_timestamps(&mut self) {
        let now = Instant::now();
        while let Some(&ts) = self.timestamps.front() {
            if now.duration_since(ts) > RATE_LIMIT_WINDOW {
                self.timestamps.pop_front();
            } else {
                break;
            }
        }
    }

    /// Check if we have capacity for an update.
    fn has_capacity(&mut self) -> bool {
        self.cleanup_old_timestamps();
        self.timestamps.len() < RATE_LIMIT_MAX_UPDATES
    }

    /// Get estimated latency for lookahead timing.
    pub fn latency(&self) -> Duration {
        self.latency_estimate
    }

    /// Update the latency estimate using exponential moving average.
    fn update_latency(&mut self, request_duration: Duration) {
        let raw_latency = request_duration / 2;
        let clamped = raw_latency.clamp(
            Duration::from_millis(MIN_LATENCY_MS),
            Duration::from_millis(MAX_LATENCY_MS),
        );

        if self.latency_estimate.is_zero() {
            self.latency_estimate = clamped;
        } else {
            let old_ms = self.latency_estimate.as_millis() as f64;
            let new_ms = clamped.as_millis() as f64;
            let averaged = LATENCY_SMOOTHING * new_ms + (1.0 - LATENCY_SMOOTHING) * old_ms;
            self.latency_estimate = Duration::from_millis(averaged as u64);
        }
    }

    /// Build a schedule of updates with lookahead batching.
    ///
    /// This looks at upcoming lyrics and determines which lines need to be
    /// batched together to respect the rate limit while showing everything
    /// at the correct time.
    pub fn build_schedule(&mut self, lines: &[TimedLine], current_position: Duration) {
        self.schedule.clear();

        // Filter to lines that haven't ended yet, sorted by start time
        let mut upcoming: Vec<_> = lines
            .iter()
            .filter(|l| l.end_time > current_position)
            .collect();
        upcoming.sort_by_key(|l| l.start_time);

        if upcoming.is_empty() {
            return;
        }

        // Track when we can next send an update
        let mut next_available = current_position;

        let mut i = 0;
        while i < upcoming.len() {
            let line = &upcoming[i];

            // If we can send at or before this line's start time, send just this line
            if next_available <= line.start_time {
                self.schedule.push_back(ScheduledUpdate {
                    display_time: line.start_time,
                    text: line.text.clone(),
                });
                next_available = line.start_time + MIN_UPDATE_INTERVAL;
                i += 1;
            } else {
                // We can't send in time for this line - need to batch with previous
                // Find all lines that would need to be batched together
                let batch_start = i;
                let mut batch_end = i + 1;

                // Keep adding lines that start before we'd have capacity again
                while batch_end < upcoming.len()
                    && upcoming[batch_end].start_time < next_available
                {
                    batch_end += 1;
                }

                // Merge these lines into the previous scheduled update
                if let Some(prev) = self.schedule.back_mut() {
                    let additional: Vec<_> = upcoming[batch_start..batch_end]
                        .iter()
                        .map(|l| l.text.as_str())
                        .collect();
                    prev.text = format!("{}{}{}", prev.text, BATCH_SEPARATOR, additional.join(BATCH_SEPARATOR));
                } else {
                    // No previous update - create one with all batched lines
                    let texts: Vec<_> = upcoming[batch_start..batch_end]
                        .iter()
                        .map(|l| l.text.as_str())
                        .collect();
                    self.schedule.push_back(ScheduledUpdate {
                        display_time: line.start_time,
                        text: texts.join(BATCH_SEPARATOR),
                    });
                    next_available = line.start_time + MIN_UPDATE_INTERVAL;
                }

                i = batch_end;
            }
        }
    }

    /// Get the next scheduled update if it's time to display it.
    pub fn get_due_update(&mut self, current_position: Duration) -> Option<String> {
        // Adjust for latency - we need to send early so it arrives on time
        let send_threshold = current_position + self.latency_estimate;
        
        if let Some(next) = self.schedule.front() {
            if send_threshold >= next.display_time {
                let update = self.schedule.pop_front().unwrap();
                return Some(update.text);
            }
        }
        None
    }

    /// Send a status update to Discord if we have capacity.
    pub async fn send_update(&mut self, text: &str, emoji: &str, token: &str) -> Result<bool> {
        // Skip if same as last sent
        if self.last_sent.as_ref() == Some(&text.to_string()) {
            return Ok(false);
        }

        if !self.has_capacity() {
            return Ok(false);
        }

        let request_duration = self.send_status(text, emoji, token).await?;
        self.update_latency(request_duration);
        self.timestamps.push_back(Instant::now());
        self.last_sent = Some(text.to_string());

        println!(
            "Discord status: \"{}\" | Latency: {}ms",
            text,
            self.latency_estimate.as_millis()
        );

        Ok(true)
    }

    /// Clear the Discord status.
    pub async fn clear_status(&mut self, token: &str) -> Result<()> {
        self.schedule.clear();
        self.last_sent = None;

        if !self.has_capacity() {
            return Ok(());
        }

        let request_duration = self.send_status("", "", token).await?;
        self.update_latency(request_duration);
        self.timestamps.push_back(Instant::now());

        println!(
            "Discord status cleared | Latency: {}ms",
            self.latency_estimate.as_millis()
        );

        Ok(())
    }

    /// Reset the schedule (e.g., when track changes).
    pub fn reset(&mut self) {
        self.schedule.clear();
        self.last_sent = None;
    }

    /// Send a status update to Discord.
    async fn send_status(&self, text: &str, emoji: &str, token: &str) -> Result<Duration> {
        let start = Instant::now();

        let response = self
            .client
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

        if !response.status().is_success() {
            return Err(DyricsError::Discord(format!(
                "Status update failed: {}",
                response.status()
            )));
        }

        Ok(start.elapsed())
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Clear the Discord status (standalone function for shutdown).
pub async fn clear_status_sync(token: &str) -> Result<()> {
    let client = Client::new();

    let response = client
        .patch("https://discord.com/api/v6/users/@me/settings")
        .header("authorization", token)
        .json(&json!({
            "custom_status": null
        }))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(DyricsError::Discord(format!(
            "Status clear failed: {}",
            response.status()
        )));
    }

    println!("Discord status cleared.");
    Ok(())
}

/// Main status update loop with lookahead batching.
pub async fn status_loop(state: PlaybackState, token: &str) -> Result<()> {
    let mut rate_limiter = RateLimiter::new();
    let mut last_track_id: Option<String> = None;
    let mut schedule_built = false;

    loop {
        let current_state = state.read().await.clone();

        match current_state {
            Some(playback) => {
                let track_id = playback.track.id.as_ref().map(|id| id.to_string());
                
                // Check if track changed
                if track_id != last_track_id {
                    last_track_id = track_id;
                    rate_limiter.reset();
                    schedule_built = false;
                }

                match &playback.lyrics {
                    Some(lyrics) => {
                        // Build schedule if not done yet
                        if !schedule_built {
                            let timed_lines = lyrics.get_timed_lines();
                            rate_limiter.build_schedule(&timed_lines, playback.position);
                            schedule_built = true;
                        }

                        // Check if there's a due update
                        if let Some(text) = rate_limiter.get_due_update(playback.position) {
                            rate_limiter.send_update(&text, "🎶", token).await?;
                        }
                    }
                    None => {
                        // No lyrics, show track info
                        let artists = playback
                            .track
                            .artists
                            .iter()
                            .map(|a| a.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        let text = format!("{} - {}", playback.track.name, artists);
                        rate_limiter.send_update(&text, "🎶", token).await?;
                    }
                }
            }
            None => {
                if last_track_id.is_some() {
                    rate_limiter.clear_status(token).await?;
                    last_track_id = None;
                    schedule_built = false;
                }
            }
        }

        // Poll frequently to catch timing precisely
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
