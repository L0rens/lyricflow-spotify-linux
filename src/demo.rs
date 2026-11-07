use crate::lyrics::{self, LyricLine};
use anyhow::{Context, Result, ensure};
use std::{fs, io::Read, path::Path, time::Instant};

const BUILTIN: &str = include_str!("../assets/demo.lrc");
const MAX_LRC_BYTES: u64 = 1_048_576;

pub struct Demo {
    pub lyrics: Vec<LyricLine>,
    pub duration_ms: u64,
    anchor: Instant,
    position_ms: u64,
    playing: bool,
}

impl Demo {
    pub fn load(path: Option<&Path>, now: Instant) -> Result<Self> {
        let source = if let Some(path) = path {
            let mut bytes = Vec::new();
            fs::File::open(path)
                .context("Could not open demo LRC file")?
                .take(MAX_LRC_BYTES + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_LRC_BYTES,
                "Demo LRC file exceeds 1 MiB"
            );
            String::from_utf8(bytes).context("Demo LRC must be UTF-8")?
        } else {
            BUILTIN.to_owned()
        };
        Self::from_lrc(&source, now)
    }

    fn from_lrc(source: &str, now: Instant) -> Result<Self> {
        let lyrics = lyrics::parse_lrc(source);
        ensure!(
            lyrics.iter().any(|line| !line.text.is_empty()),
            "Demo file has no timestamped lyrics"
        );
        let last = lyrics.last().context("Demo file has no lines")?;
        let duration_ms = if last.text.is_empty() {
            last.start_ms
        } else {
            last.words
                .last()
                .and_then(|word| word.end_ms)
                .unwrap_or_else(|| last.start_ms.saturating_add(4_000))
        }
        .max(1);
        Ok(Self {
            lyrics,
            duration_ms,
            anchor: now,
            position_ms: 0,
            playing: true,
        })
    }

    pub fn playback(&self, now: Instant) -> crate::spotify::Playback {
        use crate::spotify::{Album, Artist, DeviceState, Playback, Track};
        Playback {
            device: DeviceState::Unknown,
            is_playing: self.is_playing(now),
            progress_ms: Some(self.position(now)),
            item: Some(Track {
                id: None,
                uri: Some("lyricflow:demo".into()),
                name: "lyricflow demo".into(),
                duration_ms: self.duration_ms,
                artists: vec![Artist {
                    name: "Local LRC playback".into(),
                }],
                album: Album {
                    name: "Offline preview".into(),
                },
            }),
        }
    }

    pub fn position(&self, now: Instant) -> u64 {
        let elapsed = if self.playing {
            now.saturating_duration_since(self.anchor)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64
        } else {
            0
        };
        self.position_ms
            .saturating_add(elapsed)
            .min(self.duration_ms)
    }

    pub fn is_playing(&self, now: Instant) -> bool {
        self.playing && self.position(now) < self.duration_ms
    }

    pub fn toggle_pause(&mut self, now: Instant) {
        self.position_ms = self.position(now);
        if self.position_ms == self.duration_ms {
            self.restart(now);
            return;
        }
        self.playing = !self.playing;
        self.anchor = now;
    }

    pub fn seek(&mut self, now: Instant, delta_ms: i64) {
        self.position_ms = self
            .position(now)
            .saturating_add_signed(delta_ms)
            .min(self.duration_ms);
        self.anchor = now;
    }

    pub fn restart(&mut self, now: Instant) {
        self.position_ms = 0;
        self.anchor = now;
        self.playing = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn demo_uses_music_time_and_freezes_when_paused() {
        let now = Instant::now();
        let mut demo = Demo::load(None, now).unwrap();
        assert_eq!(demo.duration_ms, 26_000);
        assert_eq!(demo.position(now + Duration::from_millis(1_234)), 1_234);
        demo.toggle_pause(now + Duration::from_secs(2));
        assert_eq!(demo.position(now + Duration::from_secs(20)), 2_000);
        demo.toggle_pause(now + Duration::from_secs(20));
        assert_eq!(demo.position(now + Duration::from_secs(21)), 3_000);
    }
    #[test]
    fn seek_and_restart_are_clamped_and_end_can_be_replayed() {
        let now = Instant::now();
        let mut demo = Demo::load(None, now).unwrap();
        demo.seek(now, -5_000);
        assert_eq!(demo.position(now), 0);
        demo.seek(now, 100_000);
        assert_eq!(demo.position(now), demo.duration_ms);
        assert!(!demo.is_playing(now));
        demo.toggle_pause(now);
        assert_eq!(demo.position(now), 0);
        assert!(demo.is_playing(now));
    }
    #[test]
    fn malformed_files_fail_and_enhanced_end_is_preserved() {
        let now = Instant::now();
        assert!(Demo::from_lrc("no timestamps", now).is_err());
        let demo = Demo::from_lrc("[00:00]<00:00>Test<00:01.5>", now).unwrap();
        assert_eq!(demo.duration_ms, 1_500);
        assert!(!demo.lyrics[0].words.is_empty());
    }
}
