use crate::{
    lyrics::{self, LyricLine},
    spotify::{Playback, SpotifyClient, Track},
    spotify_error::SpotifyError,
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
    pub result: Result<Option<Playback>, SpotifyError>,
    pub requested_at: Instant,
    pub received_at: Instant,
}

#[derive(Default)]
struct RequestPolicy {
    retry_at: Option<Instant>,
    blocked: bool,
    failures: u32,
}

impl RequestPolicy {
    fn ready(&self, now: Instant) -> bool {
        !self.blocked && self.retry_at.is_none_or(|deadline| now >= deadline)
    }

    fn observe(&mut self, result: &Result<Option<Playback>, SpotifyError>, now: Instant) {
        match result {
            Ok(_) => *self = Self::default(),
            Err(error) if error.needs_login() || *error == SpotifyError::Forbidden => {
                self.blocked = true;
            }
            Err(SpotifyError::RateLimited(delay)) => {
                self.retry_at = now
                    .checked_add(*delay)
                    .or_else(|| now.checked_add(Duration::from_secs(60)));
            }
            Err(_) => {
                self.failures = self.failures.saturating_add(1);
                let delay = Duration::from_secs(2_u64.saturating_pow(self.failures.min(6)).min(60));
                self.retry_at = now.checked_add(delay);
            }
        }
    }
}

pub struct PlaybackWorker {
    requests: mpsc::Sender<()>,
    results: mpsc::Receiver<PlaybackSample>,
    task: JoinHandle<()>,
    pending: bool,
    policy: RequestPolicy,
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
                            Ok(Err(error)) => error
                                .downcast_ref::<SpotifyError>()
                                .cloned()
                                .unwrap_or(SpotifyError::InvalidResponse),
                            _ => SpotifyError::Timeout,
                        }),
                        requested_at,
                        received_at: Instant::now(),
                    },
                };
                if sample
                    .result
                    .as_ref()
                    .is_err_and(|error| error.needs_login())
                {
                    client = None;
                }
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
            policy: RequestPolicy::default(),
        }
    }

    pub fn retry(&mut self) {
        self.policy.blocked = false;
    }

    pub fn request(&mut self) -> bool {
        if self.pending || !self.policy.ready(Instant::now()) || self.requests.try_send(()).is_err()
        {
            return false;
        }
        self.pending = true;
        true
    }

    pub fn try_result(&mut self) -> Option<PlaybackSample> {
        let sample = self.results.try_recv().ok()?;
        self.pending = false;
        self.policy.observe(&sample.result, sample.received_at);
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

    #[test]
    fn authorization_failures_stop_polling_until_manual_retry() {
        let now = Instant::now();
        let mut policy = RequestPolicy::default();
        policy.observe(&Err(SpotifyError::Authorization), now);
        assert!(!policy.ready(now + Duration::from_secs(3_600)));
        policy.blocked = false;
        assert!(policy.ready(now));
    }

    #[test]
    fn rate_limit_cannot_be_bypassed_by_manual_retry() {
        let now = Instant::now();
        let mut policy = RequestPolicy::default();
        policy.observe(
            &Err(SpotifyError::RateLimited(Duration::from_secs(123))),
            now,
        );
        policy.blocked = false;
        assert!(!policy.ready(now + Duration::from_secs(122)));
        assert!(policy.ready(now + Duration::from_secs(123)));
        policy.observe(&Ok(None), now + Duration::from_secs(123));
        assert!(policy.ready(now + Duration::from_secs(124)));
    }

    #[test]
    fn transient_errors_back_off_and_success_resets_the_delay() {
        let now = Instant::now();
        let mut policy = RequestPolicy::default();
        for (attempt, seconds) in [(1, 2), (2, 4), (3, 8)] {
            policy.observe(&Err(SpotifyError::Unavailable), now);
            assert_eq!(policy.failures, attempt);
            assert!(!policy.ready(now + Duration::from_secs(seconds - 1)));
            assert!(policy.ready(now + Duration::from_secs(seconds)));
        }
        policy.observe(&Ok(None), now);
        assert_eq!(policy.failures, 0);
        assert!(policy.ready(now));
    }

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
            policy: RequestPolicy::default(),
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
