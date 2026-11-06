use crate::{
    lyrics::{self, LyricLine},
    spotify::{Playback, SpotifyClient, Track},
};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};

const PLAYBACK_TIMEOUT: Duration = Duration::from_secs(15);
const LYRICS_CACHE_CAPACITY: usize = 64;
type LyricsResult = Result<Option<Vec<LyricLine>>, String>;

pub struct PlaybackSample {
    pub result: Result<Option<Playback>, String>,
    pub requested_at: Instant,
    pub received_at: Instant,
}

pub struct PlaybackWorker {
    requests: mpsc::Sender<()>,
    results: mpsc::Receiver<PlaybackSample>,
    task: JoinHandle<()>,
    pending: bool,
}

impl PlaybackWorker {
    pub fn new() -> Self {
        let (requests, mut inbox) = mpsc::channel(1);
        let (outbox, results) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            let mut client = None;
            while inbox.recv().await.is_some() {
                let requested_at = Instant::now();
                let result = tokio::time::timeout(PLAYBACK_TIMEOUT, async {
                    if client.is_none() {
                        client = Some(SpotifyClient::connect().await?);
                    }
                    match client.as_mut() {
                        Some(client) => client.sample_playback().await,
                        None => unreachable!(),
                    }
                })
                .await;
                let sample = match result {
                    Ok(Ok((requested_at, received_at, playback))) => PlaybackSample {
                        result: Ok(playback),
                        requested_at,
                        received_at,
                    },
                    other => PlaybackSample {
                        result: Err(match other {
                            Ok(Err(error)) => error.to_string(),
                            _ => "Spotify request timed out".to_owned(),
                        }),
                        requested_at,
                        received_at: Instant::now(),
                    },
                };
                if outbox.send(sample).await.is_err() {
                    break;
                }
            }
        });
        Self {
            requests,
            results,
            task,
            pending: false,
        }
    }

    pub fn request(&mut self) -> bool {
        if self.pending || self.requests.try_send(()).is_err() {
            return false;
        }
        self.pending = true;
        true
    }

    pub fn try_result(&mut self) -> Option<PlaybackSample> {
        let sample = self.results.try_recv().ok()?;
        self.pending = false;
        Some(sample)
    }
}

impl Drop for PlaybackWorker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
pub struct LyricsLoader {
    pending: Option<(String, JoinHandle<LyricsResult>)>,
    cache: HashMap<String, Option<Vec<LyricLine>>>,
    order: VecDeque<String>,
}

impl LyricsLoader {
    pub fn retry(&mut self, track: &Track) -> Option<Option<Vec<LyricLine>>> {
        let key = track.key();
        self.cache.remove(&key);
        self.order.retain(|cached| cached != &key);
        self.start(track)
    }

    pub fn start(&mut self, track: &Track) -> Option<Option<Vec<LyricLine>>> {
        self.cancel();
        let key = track.key();
        if let Some(lines) = self.cache.get(&key) {
            return Some(lines.clone());
        }
        let track = track.clone();
        let task = tokio::spawn(async move {
            let artists = track
                .artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            lyrics::fetch(&track.name, &artists, &track.album.name, track.duration_ms)
                .await
                .map_err(|error| error.to_string())
        });
        self.pending = Some((key, task));
        None
    }

    pub async fn take_ready(&mut self) -> Option<(String, LyricsResult)> {
        if !self
            .pending
            .as_ref()
            .is_some_and(|(_, task)| task.is_finished())
        {
            return None;
        }
        let (key, task) = self.pending.take()?;
        let result = task
            .await
            .unwrap_or_else(|_| Err("Lyrics task failed".to_owned()));
        if let Ok(lines) = &result {
            if self.cache.len() >= LYRICS_CACHE_CAPACITY
                && let Some(oldest) = self.order.pop_front()
            {
                self.cache.remove(&oldest);
            }
            self.order.push_back(key.clone());
            self.cache.insert(key.clone(), lines.clone());
        }
        Some((key, result))
    }

    pub fn cancel(&mut self) {
        if let Some((_, task)) = self.pending.take() {
            task.abort();
        }
    }
}

impl Drop for LyricsLoader {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pending_playback_does_not_block_or_queue_duplicate_requests() {
        let (requests, mut inbox) = mpsc::channel(1);
        let (outbox, results) = mpsc::channel(1);
        let (release, wait) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            inbox.recv().await.unwrap();
            wait.await.unwrap();
            let now = Instant::now();
            outbox
                .send(PlaybackSample {
                    result: Ok(None),
                    requested_at: now,
                    received_at: now,
                })
                .await
                .unwrap();
        });
        let mut worker = PlaybackWorker {
            requests,
            results,
            task,
            pending: false,
        };
        assert!(worker.request());
        assert!(!worker.request());
        assert!(worker.try_result().is_none());
        release.send(()).unwrap();
        while !worker.task.is_finished() {
            tokio::task::yield_now().await;
        }
        assert!(worker.try_result().unwrap().result.unwrap().is_none());
        assert!(!worker.pending);
    }

    #[tokio::test]
    async fn changing_track_cancels_and_discards_inflight_lyrics() {
        let mut loader = LyricsLoader::default();
        let task = tokio::spawn(std::future::pending());
        let cancellation = task.abort_handle();
        loader.pending = Some(("old-track".to_owned(), task));
        loader.cancel();
        assert!(loader.take_ready().await.is_none());
        tokio::task::yield_now().await;
        assert!(cancellation.is_finished());
        assert!(loader.cache.is_empty());
    }

    #[tokio::test]
    async fn completed_lyrics_are_keyed_and_cache_is_bounded() {
        let mut loader = LyricsLoader::default();
        for index in 0..=LYRICS_CACHE_CAPACITY {
            let key = index.to_string();
            let task = tokio::spawn(async { Ok(None) });
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
            loader.pending = Some((key.clone(), task));
            let (returned, result) = loader.take_ready().await.unwrap();
            assert_eq!(returned, key);
            assert!(result.unwrap().is_none());
        }
        assert_eq!(loader.cache.len(), LYRICS_CACHE_CAPACITY);
        assert!(!loader.cache.contains_key("0"));
        assert!(
            loader
                .cache
                .contains_key(&LYRICS_CACHE_CAPACITY.to_string())
        );
    }

    #[tokio::test]
    async fn failed_lyrics_are_not_cached() {
        let mut loader = LyricsLoader::default();
        let task = tokio::spawn(async { Err("Temporary failure".to_owned()) });
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
        loader.pending = Some(("track".to_owned(), task));
        assert!(loader.take_ready().await.unwrap().1.is_err());
        assert!(loader.cache.is_empty());
    }

    #[tokio::test]
    async fn manual_retry_bypasses_a_cached_not_found_result() {
        let track: Track = serde_json::from_value(serde_json::json!({
            "id": "track", "name": "Song", "duration_ms": 30_000,
            "artists": [{"name": "Artist"}], "album": {"name": "Album"}
        }))
        .unwrap();
        let mut loader = LyricsLoader::default();
        let key = track.key();
        loader.cache.insert(key.clone(), None);
        loader.order.push_back(key.clone());
        assert!(matches!(loader.start(&track), Some(None)));
        assert!(loader.retry(&track).is_none());
        assert!(!loader.cache.contains_key(&key));
        assert_eq!(loader.pending.as_ref().unwrap().0, key);
        loader.cancel();
    }
}
