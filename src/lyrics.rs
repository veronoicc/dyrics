//! Lyrics types and utilities for parsing and searching synced lyrics.

use std::time::Duration;

use serde::Deserialize;
use serde_with::{serde_as, DurationSeconds};

/// Container for lyrics with timing information.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Lyrics {
    /// Start time of the lyrics.
    #[serde_as(as = "DurationSeconds<f64>")]
    pub start_time: Duration,
    /// End time of the lyrics.
    #[serde_as(as = "DurationSeconds<f64>")]
    pub end_time: Duration,
    /// The actual lyrics content (syllable-synced or line-synced).
    #[serde(flatten)]
    pub content: LyricsContent,
}

/// Different types of lyrics synchronization.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase", tag = "Type", content = "Content")]
pub enum LyricsContent {
    /// Syllable-by-syllable synced lyrics.
    Syllable(Vec<SyllableLyricsLine>),
    /// Line-by-line synced lyrics.
    Line(Vec<LineLyricsLine>),
}

/// A line of syllable-synced lyrics.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SyllableLyricsLine {
    /// Type of the line.
    pub r#type: String,
    /// Whether this line is opposite-aligned (e.g., background vocals).
    pub opposite_aligned: bool,
    /// The lead vocals with syllable timing.
    pub lead: SyllableLyricsLead,
}

/// Lead vocals with syllable-level timing.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SyllableLyricsLead {
    /// Individual syllables with timing.
    pub syllables: Vec<SyllableLyricsSyllable>,
    /// Start time of this line.
    #[serde_as(as = "DurationSeconds<f64>")]
    pub start_time: Duration,
    /// End time of this line.
    #[serde_as(as = "DurationSeconds<f64>")]
    pub end_time: Duration,
}

/// A single syllable with timing information.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SyllableLyricsSyllable {
    /// The syllable text.
    pub text: String,
    /// Whether this syllable is part of the same word as the previous.
    pub is_part_of_word: bool,
    /// Start time of this syllable.
    #[serde_as(as = "DurationSeconds<f64>")]
    pub start_time: Duration,
    /// End time of this syllable.
    #[serde_as(as = "DurationSeconds<f64>")]
    pub end_time: Duration,
}

/// A line of line-synced lyrics.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LineLyricsLine {
    /// Type of the line.
    pub r#type: String,
    /// Whether this line is opposite-aligned (e.g., background vocals).
    pub opposite_aligned: bool,
    /// The text content of the line.
    pub text: String,
    /// Start time of this line.
    #[serde_as(as = "DurationSeconds<f64>")]
    pub start_time: Duration,
    /// End time of this line.
    #[serde_as(as = "DurationSeconds<f64>")]
    pub end_time: Duration,
}

impl Lyrics {
    /// Get the current lyric text for the given timestamp.
    pub fn get_text_at(&self, timestamp: Duration) -> Option<String> {
        match &self.content {
            LyricsContent::Syllable(syllables) => {
                find_nearest_syllable_line(syllables, timestamp).map(|line| {
                    let mut result = String::new();
                    for syllable in &line.lead.syllables {
                        // Add space before syllable if it's not part of the previous word
                        if !result.is_empty() && !syllable.is_part_of_word {
                            result.push(' ');
                        }
                        result.push_str(&syllable.text);
                    }
                    result
                })
            }
            LyricsContent::Line(lines) => {
                find_nearest_line(lines, timestamp).map(|line| line.text.clone())
            }
        }
    }
}

/// Find the syllable line that contains or is nearest to the given timestamp.
fn find_nearest_syllable_line(
    lines: &[SyllableLyricsLine],
    target: Duration,
) -> Option<&SyllableLyricsLine> {
    lines.iter().min_by_key(|line| {
        let lead = &line.lead;
        if lead.start_time <= target && target <= lead.end_time {
            Duration::ZERO
        } else if target < lead.start_time {
            lead.start_time - target
        } else {
            target - lead.end_time
        }
    })
}

/// Find the line that contains or is nearest to the given timestamp.
fn find_nearest_line(lines: &[LineLyricsLine], target: Duration) -> Option<&LineLyricsLine> {
    lines.iter().min_by_key(|line| {
        if line.start_time <= target && target <= line.end_time {
            Duration::ZERO
        } else if target < line.start_time {
            line.start_time - target
        } else {
            target - line.end_time
        }
    })
}

/// A timed lyric line for lookahead processing.
#[derive(Debug, Clone)]
pub struct TimedLine {
    /// The text content of the line.
    pub text: String,
    /// Start time of this line.
    pub start_time: Duration,
    /// End time of this line.
    pub end_time: Duration,
}

impl Lyrics {
    /// Get all timed lines sorted by start time.
    pub fn get_timed_lines(&self) -> Vec<TimedLine> {
        match &self.content {
            LyricsContent::Syllable(syllables) => syllables
                .iter()
                .map(|line| {
                    let mut text = String::new();
                    for syllable in &line.lead.syllables {
                        if !text.is_empty() && !syllable.is_part_of_word {
                            text.push(' ');
                        }
                        text.push_str(&syllable.text);
                    }
                    TimedLine {
                        text,
                        start_time: line.lead.start_time,
                        end_time: line.lead.end_time,
                    }
                })
                .collect(),
            LyricsContent::Line(lines) => lines
                .iter()
                .map(|line| TimedLine {
                    text: line.text.clone(),
                    start_time: line.start_time,
                    end_time: line.end_time,
                })
                .collect(),
        }
    }
}

