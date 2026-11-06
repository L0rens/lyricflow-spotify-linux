use std::time::Instant;

const MAX_LATENCY_COMPENSATION_MS: u64 = 500;
const SEEK_THRESHOLD_MS: f64 = 750.0;
const MIN_CORRECTION_DURATION_MS: f64 = 1_000.0;
const MAX_CORRECTION_RATE: f64 = 0.5;

pub struct PlaybackClock {
    track_key: Option<String>,
    playing: bool,
    duration_ms: u64,
    anchor: Instant,
    anchor_ms: f64,
    correction_ms: f64,
    correction_duration_ms: f64,
    last_observation: Option<Instant>,
}

impl Default for PlaybackClock {
    fn default() -> Self {
        Self {
            track_key: None,
            playing: false,
            duration_ms: 0,
            anchor: Instant::now(),
            anchor_ms: 0.0,
            correction_ms: 0.0,
            correction_duration_ms: MIN_CORRECTION_DURATION_MS,
            last_observation: None,
        }
    }
}

impl PlaybackClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn position(&self, now: Instant) -> u64 {
        self.precise_position(now) as u64
    }

    fn precise_position(&self, now: Instant) -> f64 {
        if self.track_key.is_none() {
            return 0.0;
        }
        let elapsed_ms = if self.playing {
            now.saturating_duration_since(self.anchor).as_secs_f64() * 1_000.0
        } else {
            0.0
        };
        let correction =
            self.correction_ms * (elapsed_ms / self.correction_duration_ms).clamp(0.0, 1.0);
        (self.anchor_ms + elapsed_ms + correction).clamp(0.0, self.duration_ms as f64)
    }

    pub fn sync(
        &mut self,
        track_key: Option<&str>,
        playing: bool,
        progress_ms: u64,
        duration_ms: u64,
        request_started: Instant,
        received_at: Instant,
    ) {
        if self
            .last_observation
            .is_some_and(|previous| received_at < previous)
        {
            return;
        }
        let Some(track_key) = track_key else {
            self.clear();
            self.last_observation = Some(received_at);
            return;
        };
        let half_rtt_ms = (received_at
            .saturating_duration_since(request_started)
            .as_millis()
            / 2)
        .min(u128::from(MAX_LATENCY_COMPENSATION_MS)) as u64;
        let compensation_ms = if playing { half_rtt_ms } else { 0 };
        let reported_ms = progress_ms.saturating_add(compensation_ms).min(duration_ms) as f64;
        let track_changed = self.track_key.as_deref() != Some(track_key);
        let state_changed = self.playing != playing;
        let predicted_ms = self.precise_position(received_at);
        let error_ms = reported_ms - predicted_ms;
        self.last_observation = Some(received_at);
        self.duration_ms = duration_ms;

        if track_changed
            || state_changed
            || !playing
            || error_ms.abs() >= SEEK_THRESHOLD_MS + half_rtt_ms as f64
        {
            self.track_key = Some(track_key.to_owned());
            self.playing = playing;
            self.anchor = received_at;
            self.anchor_ms = reported_ms;
            self.correction_ms = 0.0;
        } else if error_ms.abs() > half_rtt_ms as f64 {
            self.anchor = received_at;
            self.anchor_ms = predicted_ms;
            self.correction_ms = error_ms;
            self.correction_duration_ms =
                (error_ms.abs() / MAX_CORRECTION_RATE).max(MIN_CORRECTION_DURATION_MS);
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const TRACK_DURATION_MS: u64 = 180_000;

    fn sample(clock: &mut PlaybackClock, key: &str, playing: bool, progress_ms: u64, now: Instant) {
        clock.sync(Some(key), playing, progress_ms, TRACK_DURATION_MS, now, now);
    }

    #[test]
    fn extrapolates_and_clamps_to_track_duration() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        sample(&mut clock, "track", true, 1_000, now);
        assert_eq!(clock.position(now + Duration::from_millis(1_234)), 2_234);
        assert_eq!(clock.position(now + Duration::from_secs(200)), 180_000);
    }

    #[test]
    fn pause_freezes_and_resume_reanchors_authoritatively() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        sample(&mut clock, "track", true, 1_000, now);
        sample(
            &mut clock,
            "track",
            false,
            1_900,
            now + Duration::from_secs(1),
        );
        assert_eq!(clock.position(now + Duration::from_secs(20)), 1_900);
        sample(
            &mut clock,
            "track",
            true,
            1_900,
            now + Duration::from_secs(20),
        );
        assert_eq!(clock.position(now + Duration::from_secs(21)), 2_900);
    }

    #[test]
    fn small_forward_drift_is_slewed_without_a_poll_jump() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        sample(&mut clock, "track", true, 1_000, now);
        let observation = now + Duration::from_secs(1);
        sample(&mut clock, "track", true, 2_200, observation);
        assert_eq!(clock.position(observation), 2_000);
        assert_eq!(
            clock.position(observation + Duration::from_millis(500)),
            2_600
        );
        assert_eq!(clock.position(observation + Duration::from_secs(1)), 3_200);
        assert_eq!(
            clock.position(observation + Duration::from_secs(10)),
            12_200
        );
    }

    #[test]
    fn small_backward_drift_slows_without_moving_backwards() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        sample(&mut clock, "track", true, 1_000, now);
        let observation = now + Duration::from_secs(1);
        sample(&mut clock, "track", true, 1_400, observation);
        assert_eq!(clock.position(observation), 2_000);
        let mut previous = 2_000;
        for step in 1..=24 {
            let current = clock.position(observation + Duration::from_millis(step * 100));
            assert!(current >= previous);
            previous = current;
        }
        assert_eq!(
            clock.position(observation + Duration::from_millis(1_200)),
            2_600
        );
    }

    #[test]
    fn seeks_in_both_directions_apply_immediately() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        sample(&mut clock, "track", true, 20_000, now);
        sample(
            &mut clock,
            "track",
            true,
            40_000,
            now + Duration::from_secs(1),
        );
        assert_eq!(clock.position(now + Duration::from_secs(1)), 40_000);
        sample(
            &mut clock,
            "track",
            true,
            5_000,
            now + Duration::from_secs(2),
        );
        assert_eq!(clock.position(now + Duration::from_secs(2)), 5_000);
    }

    #[test]
    fn a_small_seek_while_paused_is_not_smoothed() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        sample(&mut clock, "track", false, 20_000, now);
        sample(
            &mut clock,
            "track",
            false,
            20_100,
            now + Duration::from_secs(1),
        );
        assert_eq!(clock.position(now + Duration::from_secs(10)), 20_100);
    }

    #[test]
    fn track_change_discards_pending_drift_correction() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        sample(&mut clock, "first-id", true, 20_000, now);
        sample(
            &mut clock,
            "first-id",
            true,
            21_300,
            now + Duration::from_secs(1),
        );
        sample(
            &mut clock,
            "second-id",
            true,
            100,
            now + Duration::from_millis(1_500),
        );
        assert_eq!(clock.position(now + Duration::from_millis(1_500)), 100);
        assert_eq!(clock.position(now + Duration::from_millis(2_500)), 1_100);
    }

    #[test]
    fn midpoint_rtt_compensation_is_bounded_and_disabled_when_paused() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        let received = now + Duration::from_millis(200);
        clock.sync(Some("a"), true, 1_000, TRACK_DURATION_MS, now, received);
        assert_eq!(clock.position(received), 1_100);
        let slow_received = now + Duration::from_secs(10);
        clock.sync(
            Some("b"),
            true,
            1_000,
            TRACK_DURATION_MS,
            now,
            slow_received,
        );
        assert_eq!(clock.position(slow_received), 1_500);
        clock.sync(
            Some("b"),
            false,
            1_000,
            TRACK_DURATION_MS,
            now,
            slow_received,
        );
        assert_eq!(clock.position(slow_received), 1_000);
    }

    #[test]
    fn observations_inside_network_uncertainty_do_not_reanchor() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        sample(&mut clock, "track", true, 1_000, now);
        let received = now + Duration::from_secs(1);
        clock.sync(
            Some("track"),
            true,
            1_850,
            TRACK_DURATION_MS,
            received - Duration::from_millis(200),
            received,
        );
        assert_eq!(clock.position(received), 2_000);
        assert_eq!(clock.position(received + Duration::from_secs(1)), 3_000);
    }

    #[test]
    fn repeated_samples_do_not_accumulate_drift() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        sample(&mut clock, "track", true, 1_000, now);
        for seconds in 1..=100 {
            let received = now + Duration::from_secs(seconds);
            sample(&mut clock, "track", true, 1_200 + seconds * 1_000, received);
        }
        assert_eq!(clock.position(now + Duration::from_secs(101)), 102_200);
    }

    #[test]
    fn ignores_stale_responses() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        sample(
            &mut clock,
            "new",
            true,
            10_000,
            now + Duration::from_secs(2),
        );
        sample(&mut clock, "old", true, 1_000, now + Duration::from_secs(1));
        assert_eq!(clock.position(now + Duration::from_secs(3)), 11_000);
    }

    #[test]
    fn clearing_and_missing_tracks_stop_extrapolation() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        sample(&mut clock, "track", true, 1_000, now);
        clock.clear();
        assert_eq!(clock.position(now + Duration::from_secs(1)), 0);
        sample(&mut clock, "track", true, 1_000, now);
        clock.sync(None, false, 0, 0, now, now + Duration::from_secs(1));
        assert_eq!(clock.position(now + Duration::from_secs(2)), 0);
    }

    #[test]
    fn reported_positions_and_latency_cannot_exceed_duration() {
        let now = Instant::now();
        let mut clock = PlaybackClock::new();
        clock.sync(Some("track"), true, u64::MAX, 100, now, now);
        assert_eq!(clock.position(now), 100);
        clock.sync(
            Some("next"),
            true,
            90,
            100,
            now,
            now + Duration::from_millis(100),
        );
        assert_eq!(clock.position(now + Duration::from_millis(100)), 100);
    }
}
