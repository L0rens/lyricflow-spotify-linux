use crate::*;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, buffer::Cell};
use serde_json::json;

const MUTED: Color = Color::Rgb(175, 175, 175);

fn lyric(start_ms: u64, text: &str) -> LyricLine {
    LyricLine {
        start_ms,
        text: text.to_owned(),
        words: Vec::new(),
    }
}

fn app_at(position_ms: u64, lyrics: Vec<LyricLine>) -> App {
    let playback: Playback = serde_json::from_value(json!({
        "is_playing": false,
        "progress_ms": position_ms,
        "item": {
            "id": "test-track",
            "name": "Fixture song",
            "duration_ms": 30_000,
            "artists": [{"name": "Fixture artist"}],
            "album": {"name": "Fixture album"}
        }
    }))
    .unwrap();
    let settings = Settings {
        theme: THEMES
            .iter()
            .position(|theme| theme.0 == "Pure White")
            .unwrap(),
        highlight_color: 4,
        ..Settings::default()
    };
    let mut app = App {
        playback: None,
        clock: PlaybackClock::new(),
        message: String::new(),
        connected: true,
        track_key: Some("test-track".to_owned()),
        lyrics: Some(lyrics),
        lyrics_message: String::new(),
        settings,
        menu_open: false,
        menu_selection: 0,
        update_message: String::new(),
    };
    let now = Instant::now();
    let playback = Some(playback);
    app.sync_clock(&playback, now, now);
    app.playback = playback;
    app
}

fn lyrics_buffer(app: &App) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
    terminal
        .draw(|frame| draw_lyrics(frame, app, frame.area(), MUTED))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn matching_cells<'a>(buffer: &'a Buffer, text: &str) -> Vec<&'a Cell> {
    let area = buffer.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if x as usize + text.len() > area.right() as usize {
                continue;
            }
            let cells: Vec<_> = (0..text.len())
                .map(|offset| &buffer[(x + offset as u16, y)])
                .collect();
            let found: String = cells.iter().map(|cell| cell.symbol()).collect();
            if found == text {
                return cells;
            }
        }
    }
    panic!("Text {text:?} is missing from the rendered terminal");
}

#[test]
fn white_karaoke_starts_muted_and_fills_only_elapsed_part() {
    let lyrics = vec![lyric(1_000, "ABCD"), lyric(5_000, "WXYZ")];
    let at_start = lyrics_buffer(&app_at(1_000, lyrics.clone()));
    assert!(
        matching_cells(&at_start, "ABCD")
            .iter()
            .all(|cell| cell.fg == MUTED)
    );
    let halfway = lyrics_buffer(&app_at(3_000, lyrics));
    let cells = matching_cells(&halfway, "ABCD");
    assert_eq!(
        cells.iter().map(|cell| cell.fg).collect::<Vec<_>>(),
        vec![Color::White, Color::White, MUTED, MUTED]
    );
}

#[test]
fn short_phrase_finishes_before_a_long_gap_without_advancing_focus() {
    let text = "Whatever it takes";
    let context = "abcdefghijklmnopqrstuvwxyzABCDEF";
    let app = app_at(
        4_000,
        vec![
            lyric(1_000, text),
            lyric(13_000, context),
            lyric(17_000, context),
            lyric(21_000, context),
            lyric(25_000, context),
        ],
    );
    let buffer = lyrics_buffer(&app);
    assert!(
        matching_cells(&buffer, text)
            .iter()
            .all(|cell| cell.fg == Color::White)
    );
    assert!(
        matching_cells(&buffer, context)
            .iter()
            .all(|cell| cell.fg != Color::White)
    );
}

#[test]
fn first_line_is_not_highlighted_before_its_timestamp_even_without_fill() {
    let mut app = app_at(0, vec![lyric(1_000, "ABCD")]);
    app.settings.karaoke_fill = false;
    let buffer = lyrics_buffer(&app);
    assert!(
        matching_cells(&buffer, "ABCD")
            .iter()
            .all(|cell| cell.fg != Color::White)
    );
}

#[test]
fn positive_offset_selects_the_later_line_at_the_adjusted_position() {
    let mut app = app_at(
        3_000,
        vec![
            lyric(1_000, "ABCD"),
            lyric(5_000, "WXYZ"),
            lyric(9_000, "IJKL"),
        ],
    );
    app.settings.lyrics_offset_ms = 3_000;
    let buffer = lyrics_buffer(&app);
    let cells = matching_cells(&buffer, "WXYZ");
    assert_eq!(cells[0].fg, Color::White);
    assert_eq!(cells[1].fg, MUTED);
    assert!(
        matching_cells(&buffer, "ABCD")
            .iter()
            .all(|cell| cell.fg != Color::White)
    );
}

#[test]
fn negative_offset_saturates_before_track_start() {
    let mut app = app_at(1_000, vec![lyric(1_000, "ABCD")]);
    app.settings.lyrics_offset_ms = -5_000;
    let buffer = lyrics_buffer(&app);
    assert!(
        matching_cells(&buffer, "ABCD")
            .iter()
            .all(|cell| cell.fg != Color::White)
    );
}

#[test]
fn timestamped_empty_line_clears_the_active_text_during_instrumental() {
    let app = app_at(
        6_000,
        vec![
            lyric(1_000, "ABCD"),
            lyric(5_000, ""),
            lyric(10_000, "WXYZ"),
        ],
    );
    let buffer = lyrics_buffer(&app);
    assert!(
        matching_cells(&buffer, "ABCD")
            .iter()
            .all(|cell| cell.fg != Color::White)
    );
    assert!(
        matching_cells(&buffer, "WXYZ")
            .iter()
            .all(|cell| cell.fg != Color::White)
    );
}

#[test]
fn paused_playback_keeps_rendered_karaoke_unchanged() {
    let app = app_at(2_500, vec![lyric(1_000, "ABCD"), lyric(5_000, "WXYZ")]);
    assert_eq!(lyrics_buffer(&app), lyrics_buffer(&app));
}

#[test]
fn legacy_settings_preserve_values_and_supply_animation_defaults() {
    let old = json!({
        "lyrics_offset_ms": -400,
        "refresh_seconds": 3,
        "show_lyrics": false,
        "highlight_color": 2,
        "theme": 7,
        "language": "ru",
        "check_updates": false
    });
    let settings: Settings = serde_json::from_value(old).unwrap();
    assert_eq!(settings.lyrics_offset_ms, -400);
    assert_eq!(settings.refresh_seconds, 3);
    assert!(!settings.show_lyrics);
    assert_eq!(settings.highlight_color, 2);
    assert_eq!(settings.theme, 7);
    assert_eq!(settings.language, "ru");
    assert!(!settings.check_updates);
    assert!(settings.animation);
    assert_eq!(settings.animation_fps, 30);
    assert!(settings.karaoke_fill);
    assert_eq!(settings.text_size, 1);
    assert_eq!(settings.font_size_pt, 0);
}

#[test]
fn all_existing_settings_roundtrip_without_losing_user_choices() {
    let saved = json!({
        "lyrics_offset_ms": 2_300,
        "refresh_seconds": 7,
        "show_lyrics": false,
        "highlight_color": 4,
        "theme": 11,
        "language": "ja",
        "check_updates": false,
        "animation": false,
        "animation_fps": 45,
        "karaoke_fill": false,
        "text_size": 2,
        "font_size_pt": 21
    });
    let settings: Settings = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(serde_json::to_value(settings).unwrap(), saved);
}

#[test]
fn pure_white_preserves_terminal_background_in_the_main_view() {
    let app = app_at(3_000, vec![lyric(1_000, "ABCD"), lyric(5_000, "WXYZ")]);
    let mut terminal = Terminal::new(TestBackend::new(90, 40)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .all(|cell| cell.bg == Color::Reset)
    );
}

#[test]
fn pure_white_popup_selection_preserves_terminal_background() {
    let mut app = app_at(3_000, vec![lyric(1_000, "ABCD"), lyric(5_000, "WXYZ")]);
    app.menu_open = true;
    let mut terminal = Terminal::new(TestBackend::new(90, 40)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let buffer = terminal.backend().buffer();
    assert!(
        matching_cells(buffer, "MENU")
            .iter()
            .all(|cell| cell.bg == Color::Reset)
    );
    assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
}

#[test]
fn missing_progress_does_not_reset_the_clock_for_the_same_track() {
    let mut app = app_at(3_000, vec![lyric(1_000, "ABCD")]);
    let mut playback = app.playback.clone();
    playback.as_mut().unwrap().progress_ms = None;
    let now = Instant::now();
    app.sync_clock(&playback, now, now);
    assert_eq!(app.position(), 3_000);
}

#[test]
fn different_spotify_ids_reset_the_clock_despite_identical_metadata() {
    let mut app = app_at(20_000, vec![lyric(1_000, "ABCD")]);
    let mut data = json!({
        "is_playing": false,
        "progress_ms": 100,
        "item": {
            "id": "another-track",
            "name": "Fixture song",
            "duration_ms": 30_000,
            "artists": [{"name": "Fixture artist"}],
            "album": {"name": "Fixture album"}
        }
    });
    let playback = Some(serde_json::from_value::<Playback>(data.take()).unwrap());
    let now = Instant::now();
    app.sync_clock(&playback, now, now);
    assert_eq!(app.position(), 100);
}

#[test]
fn absent_playback_clears_the_clock() {
    let mut app = app_at(20_000, vec![lyric(1_000, "ABCD")]);
    let now = Instant::now();
    app.sync_clock(&None, now, now);
    assert_eq!(app.position(), 0);
}
