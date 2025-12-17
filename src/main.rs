//! Dyrics - Discord lyrics status updater for Spotify.

use std::sync::Arc;

use tokio::sync::RwLock;

use dyrics::{config::Config, discord, error::Result, spotify};

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load()?;
    let token = config.discord.token.clone();

    let spotify_client = spotify::create_client(&config.spotify).await?;

    let playback_state = Arc::new(RwLock::new(None));

    // Spawn the playback position stepper
    tokio::spawn(spotify::step_loop(playback_state.clone()));

    // Set up Ctrl+C handler to clear status on shutdown
    let shutdown_token = token.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
        println!("\nShutting down, clearing Discord status...");
        if let Err(e) = discord::clear_status_sync(&shutdown_token).await {
            eprintln!("Failed to clear status: {e}");
        }
        std::process::exit(0);
    });

    // Run sync and status loops concurrently
    tokio::try_join!(
        spotify::resync_loop(
            playback_state.clone(),
            spotify_client,
            config.spotify.resync_interval
        ),
        discord::status_loop(playback_state.clone(), &token),
    )?;

    Ok(())
}