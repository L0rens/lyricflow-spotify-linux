mod cli;
mod clock;
mod i18n;
mod karaoke;
mod lyrics;
mod services;
mod spotify;
mod updates;

#[cfg(test)]
mod tests;

use anyhow::Result;
use clock::PlaybackClock;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use karaoke::blend;
use lyrics::LyricLine;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, Paragraph},
};
use serde::{Deserialize, Serialize};
use services::{LyricsLoader, PlaybackWorker};
use spotify::{Playback, SpotifyClient};
use std::time::{Duration, Instant};
use std::{fs, path::PathBuf};

const CYAN: Color = Color::Rgb(131, 217, 245);
const GREEN: Color = Color::Rgb(105, 221, 163);

type Rgb = (u8, u8, u8);
type ThemeDefinition = (&'static str, Rgb, Rgb, Rgb, Rgb);

const THEMES: &[ThemeDefinition] = &[
    (
        "Default",
        (131, 217, 245),
        (105, 221, 163),
        (124, 135, 149),
        (65, 76, 91),
    ),
    (
        "Dracula",
        (189, 147, 249),
        (80, 250, 123),
        (139, 147, 164),
        (98, 114, 164),
    ),
    (
        "Nord",
        (136, 192, 208),
        (163, 190, 140),
        (129, 161, 193),
        (76, 86, 106),
    ),
    (
        "Gruvbox",
        (250, 189, 47),
        (184, 187, 38),
        (168, 153, 132),
        (102, 92, 84),
    ),
    (
        "Catppuccin Mocha",
        (203, 166, 247),
        (166, 227, 161),
        (166, 173, 200),
        (88, 91, 112),
    ),
    (
        "Tokyo Night",
        (122, 162, 247),
        (158, 206, 106),
        (130, 137, 172),
        (65, 72, 104),
    ),
    (
        "Rose Pine",
        (196, 167, 231),
        (156, 207, 216),
        (144, 140, 170),
        (82, 79, 103),
    ),
    (
        "Everforest",
        (167, 192, 128),
        (131, 192, 146),
        (157, 169, 160),
        (83, 100, 87),
    ),
    (
        "Solarized Dark",
        (38, 139, 210),
        (133, 153, 0),
        (101, 123, 131),
        (88, 110, 117),
    ),
    (
        "Solarized Light",
        (38, 139, 210),
        (133, 153, 0),
        (88, 110, 117),
        (147, 161, 161),
    ),
    (
        "One Dark",
        (97, 175, 239),
        (152, 195, 121),
        (130, 137, 151),
        (75, 82, 99),
    ),
    (
        "Monokai",
        (102, 217, 239),
        (166, 226, 46),
        (117, 113, 94),
        (89, 86, 74),
    ),
    (
        "Kanagawa",
        (126, 156, 216),
        (152, 187, 108),
        (114, 113, 105),
        (84, 84, 109),
    ),
    (
        "Ayu Mirage",
        (115, 190, 250),
        (170, 217, 76),
        (112, 122, 139),
        (79, 86, 99),
    ),
    (
        "Cyberpunk",
        (0, 245, 255),
        (255, 50, 180),
        (145, 143, 190),
        (101, 51, 122),
    ),
    (
        "Matrix",
        (50, 255, 100),
        (0, 200, 70),
        (70, 130, 80),
        (40, 90, 50),
    ),
    (
        "Pure White",
        (255, 255, 255),
        (245, 245, 245),
        (180, 180, 180),
        (255, 255, 255),
    ),
];

fn is_white_theme(index: usize) -> bool {
    THEMES[index % THEMES.len()].0 == "Pure White"
}

fn foreground(_theme: usize) -> Color {
    Color::White
}

fn background(_theme: usize) -> Color {
    Color::Reset
}

fn theme_color(index: usize, slot: usize) -> Color {
    let theme = &THEMES[index % THEMES.len()];
    let (r, g, b) = match slot {
        0 => theme.1,
        1 => theme.2,
        2 => theme.3,
        _ => theme.4,
    };
    Color::Rgb(r, g, b)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    lyrics_offset_ms: i64,
    refresh_seconds: u64,
    show_lyrics: bool,
    highlight_color: usize,
    theme: usize,
    language: String,
    check_updates: bool,
    animation: bool,
    animation_fps: u64,
    karaoke_fill: bool,
    text_size: usize,
    font_size_pt: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lyrics_offset_ms: 0,
            refresh_seconds: 5,
            show_lyrics: true,
            highlight_color: 0,
            theme: 0,
            language: "en".to_owned(),
            check_updates: true,
            animation: true,
            animation_fps: 30,
            karaoke_fill: true,
            text_size: 1,
            font_size_pt: 0,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".config/lyricflow/settings.json"))
}

fn load_settings() -> Settings {
    settings_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_settings(settings: &Settings) -> Result<()> {
    if let Some(path) = settings_path() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(settings)?)?;
    }
    Ok(())
}

fn apply_kitty_font_size(size: u8) -> std::result::Result<(), String> {
    if std::env::var_os("KITTY_WINDOW_ID").is_none() {
        return Err("Font size requires Kitty".to_owned());
    }
    let output = std::process::Command::new("kitty")
        .args(["@", "set-font-size", &size.to_string()])
        .output()
        .map_err(|error| format!("Kitty font control: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(format!("Kitty remote control unavailable: {reason}"))
    }
}

fn highlight_color(index: usize, theme: usize) -> Color {
    if is_white_theme(theme) {
        return Color::White;
    }
    match index % 5 {
        0 => GREEN,
        1 => CYAN,
        2 => Color::Yellow,
        3 => Color::Magenta,
        _ => Color::White,
    }
}

fn centered_rect(width: u16, height: u16, area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    ratatui::layout::Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_menu(frame: &mut Frame, app: &App) {
    let cyan = theme_color(app.settings.theme, 0);
    let muted = if is_white_theme(app.settings.theme) {
        Color::Rgb(175, 175, 175)
    } else {
        theme_color(app.settings.theme, 2)
    };
    let area = centered_rect(76, 30, frame.area());
    frame.render_widget(Clear, area);
    let t = |key| i18n::tr(&app.settings.language, key);
    let mut items = vec![
        (
            t("lyrics_offset"),
            format!("{:+} ms", app.settings.lyrics_offset_ms),
        ),
        (
            t("spotify_refresh"),
            format!("{} s", app.settings.refresh_seconds),
        ),
        (
            t("show_lyrics"),
            if app.settings.show_lyrics {
                "ON"
            } else {
                "OFF"
            }
            .to_string(),
        ),
        (
            t("highlight_color"),
            ["Green", "Cyan", "Yellow", "Magenta", "White"][app.settings.highlight_color % 5]
                .to_string(),
        ),
        (
            t("color_theme"),
            THEMES[app.settings.theme % THEMES.len()].0.to_string(),
        ),
        (
            t("language"),
            i18n::language_name(&app.settings.language).to_owned(),
        ),
        (
            t("check_updates"),
            if app.settings.check_updates {
                "ON"
            } else {
                "OFF"
            }
            .to_owned(),
        ),
        (
            "Animation",
            if app.settings.animation { "ON" } else { "OFF" }.to_owned(),
        ),
        ("Animation FPS", format!("{}", app.settings.animation_fps)),
        (
            "Karaoke fill",
            if app.settings.karaoke_fill {
                "ON"
            } else {
                "OFF"
            }
            .to_owned(),
        ),
        (
            "Lyrics spacing",
            ["Compact", "Normal", "Spacious"][app.settings.text_size.min(2)].to_owned(),
        ),
        (
            "Font size (Kitty)",
            if app.settings.font_size_pt == 0 {
                "Terminal default".to_owned()
            } else {
                format!("{} pt", app.settings.font_size_pt)
            },
        ),
        (t("check_now"), "Enter".to_owned()),
        (t("save_settings"), "Enter".to_string()),
        (t("reset_defaults"), "Enter".to_string()),
        (t("close_menu"), "Esc".to_string()),
    ];
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}  /  lyricflow", t("settings")),
            Style::default().fg(cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (index, (name, value)) in items.drain(..).enumerate() {
        let selected = index == app.menu_selection;
        let marker = if selected { "  > " } else { "    " };
        let label = format!("{marker}{name}  :  {value}");
        let style = if selected && is_white_theme(app.settings.theme) {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else if selected {
            Style::default()
                .fg(Color::Black)
                .bg(cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(foreground(app.settings.theme))
        };
        lines.push(Line::from(Span::styled(label, style)));
    }
    lines.push(Line::from(Span::styled(
        app.update_message.as_str(),
        Style::default().fg(cyan),
    )));
    lines.push(Line::from(Span::styled(
        t("menu_help"),
        Style::default().fg(muted),
    )));
    lines.push(Line::from(Span::styled(
        t("menu_help2"),
        Style::default().fg(muted),
    )));
    frame.render_widget(
        Paragraph::new(lines).block(panel("MENU", app.settings.theme)),
        area,
    );
}

fn edit_setting(app: &mut App, direction: i64) {
    match app.menu_selection {
        0 => {
            app.settings.lyrics_offset_ms =
                (app.settings.lyrics_offset_ms + direction * 100).clamp(-10_000, 10_000)
        }
        1 => {
            app.settings.refresh_seconds =
                (app.settings.refresh_seconds as i64 + direction).clamp(1, 30) as u64
        }
        2 => app.settings.show_lyrics = !app.settings.show_lyrics,
        3 => {
            app.settings.highlight_color =
                (app.settings.highlight_color as i64 + direction).rem_euclid(5) as usize
        }
        4 => {
            app.settings.theme =
                (app.settings.theme as i64 + direction).rem_euclid(THEMES.len() as i64) as usize
        }
        5 => {
            app.settings.language =
                i18n::next_language(&app.settings.language, direction).to_owned()
        }
        6 => app.settings.check_updates = !app.settings.check_updates,
        7 => app.settings.animation = !app.settings.animation,
        8 => {
            app.settings.animation_fps =
                (app.settings.animation_fps as i64 + direction * 5).clamp(10, 60) as u64
        }
        9 => app.settings.karaoke_fill = !app.settings.karaoke_fill,
        10 => {
            app.settings.text_size =
                (app.settings.text_size as i64 + direction).rem_euclid(3) as usize
        }
        11 => {
            let current = if app.settings.font_size_pt == 0 {
                16
            } else {
                app.settings.font_size_pt
            };
            let next = (i64::from(current) + direction).clamp(10, 30) as u8;
            match apply_kitty_font_size(next) {
                Ok(()) => {
                    app.settings.font_size_pt = next;
                    app.update_message = format!("Kitty font size: {next} pt");
                }
                Err(error) => app.update_message = error,
            }
        }
        _ => {}
    }
}

struct App {
    playback: Option<Playback>,
    clock: PlaybackClock,
    message: String,
    connected: bool,
    track_key: Option<String>,
    lyrics: Option<Vec<LyricLine>>,
    lyrics_message: String,
    settings: Settings,
    menu_open: bool,
    menu_selection: usize,
    update_message: String,
}

impl App {
    fn new() -> Self {
        Self {
            playback: None,
            clock: PlaybackClock::new(),
            message: String::from("Connecting to Spotify..."),
            connected: false,
            track_key: None,
            lyrics: None,
            lyrics_message: String::from("Waiting for a track..."),
            settings: load_settings(),
            menu_open: false,
            menu_selection: 0,
            update_message: String::new(),
        }
    }

    fn position(&self) -> u64 {
        self.clock.position(Instant::now())
    }

    fn sync_clock(
        &mut self,
        playback: &Option<Playback>,
        requested_at: Instant,
        received_at: Instant,
    ) {
        match playback
            .as_ref()
            .and_then(|state| state.item.as_ref().map(|track| (state, track)))
        {
            Some((state, track)) => {
                let key = track.key();
                let (progress_ms, requested_at) = match state.progress_ms {
                    Some(progress) => (progress, requested_at),
                    None => {
                        let same_track = self
                            .playback
                            .as_ref()
                            .and_then(|state| state.item.as_ref())
                            .is_some_and(|previous| previous.key() == key);
                        (
                            if same_track {
                                self.clock.position(received_at)
                            } else {
                                0
                            },
                            received_at,
                        )
                    }
                };
                self.clock.sync(
                    Some(&key),
                    state.is_playing,
                    progress_ms,
                    track.duration_ms,
                    requested_at,
                    received_at,
                );
            }
            None => self
                .clock
                .sync(None, false, 0, 0, requested_at, received_at),
        }
    }

    fn set_lyrics(&mut self, result: Result<Option<Vec<LyricLine>>, String>) {
        match result {
            Ok(Some(lines)) => self.lyrics = Some(lines),
            Ok(None) => self.lyrics_message = "No synchronized lyrics found".to_owned(),
            Err(error) => self.lyrics_message = format!("Lyrics unavailable: {error}"),
        }
    }
}

fn timestamp(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn panel(title: &str, theme: usize) -> Block<'_> {
    Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(theme_color(theme, 0))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme_color(theme, 3)))
        .style(Style::default().bg(background(theme)).fg(foreground(theme)))
}

fn draw_lyrics(frame: &mut Frame, app: &App, area: ratatui::layout::Rect, muted: Color) {
    let t = |key| i18n::tr(&app.settings.language, key);
    let height = area.height.saturating_sub(2) as usize;
    let mut rows = vec![Line::from(""); height];
    let mut title = t("lyrics").to_owned();
    if !app.settings.show_lyrics {
        if height > 0 {
            rows[height / 2] = Line::from(t("lyrics_disabled"));
        }
    } else if let Some(lyrics) = &app.lyrics {
        if !lyrics.is_empty() && height > 0 {
            let position = app
                .position()
                .saturating_add_signed(app.settings.lyrics_offset_ms);
            let active = lyrics.partition_point(|line| line.start_ms <= position);
            let current = active.saturating_sub(1);
            let accent = highlight_color(app.settings.highlight_color, app.settings.theme);
            let start = lyrics[current].start_ms;
            let next = lyrics
                .get(current + 1)
                .map(|line| line.start_ms)
                .unwrap_or_else(|| {
                    app.playback
                        .as_ref()
                        .and_then(|p| p.item.as_ref())
                        .map_or(start.saturating_add(8_000), |track| track.duration_ms)
                });
            title = format!(
                "{} · {}",
                t("lyrics"),
                if lyrics[current].words.is_empty() {
                    "Approximate fill"
                } else {
                    "Word timestamps"
                }
            );
            let transition = if app.settings.animation {
                ((position as f64 - next.saturating_sub(180) as f64) / 180.0).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let stride = match app.settings.text_size.min(2) {
                0 => 1,
                1 => 1,
                _ => 2,
            };
            let center = height / 2;
            for (row, output) in rows.iter_mut().enumerate() {
                let delta = row as isize - center as isize;
                if delta.rem_euclid(stride) != 0 {
                    continue;
                }
                let index = current as isize + delta.div_euclid(stride);
                if index < 0 || index as usize >= lyrics.len() {
                    continue;
                }
                let index = index as usize;
                let line = &lyrics[index];
                if index == current && active > 0 {
                    *output = karaoke::line(
                        line,
                        position,
                        karaoke::fill_end_ms(lyrics, current, next),
                        muted,
                        accent,
                        app.settings.animation && app.settings.karaoke_fill,
                    );
                } else {
                    let distance = index.abs_diff(current);
                    let base = if distance == 1 {
                        0.85
                    } else if distance == 2 {
                        0.68
                    } else {
                        0.50
                    };
                    let glow = if app.settings.animation && index == current + 1 {
                        transition * 0.35
                    } else {
                        0.0
                    };
                    *output = Line::from(Span::styled(
                        line.text.clone(),
                        Style::default().fg(blend(Color::Rgb(85, 85, 85), muted, base + glow)),
                    ));
                }
            }
        }
    } else if height > 0 {
        rows[height / 2] = Line::from(Span::styled(
            app.lyrics_message.clone(),
            Style::default().fg(muted),
        ));
    }
    frame.render_widget(
        Paragraph::new(rows)
            .alignment(Alignment::Center)
            .block(panel(&title, app.settings.theme)),
        area,
    );
}

fn draw(frame: &mut Frame, app: &App) {
    let t = |key| i18n::tr(&app.settings.language, key);
    let cyan = theme_color(app.settings.theme, 0);
    let green = theme_color(app.settings.theme, 1);
    let muted = if is_white_theme(app.settings.theme) {
        Color::Rgb(175, 175, 175)
    } else {
        theme_color(app.settings.theme, 2)
    };
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(9),
            Constraint::Min(7),
            Constraint::Length(3),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " ♪ lyricflow ",
                Style::default().fg(cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(t("tagline"), Style::default().fg(muted)),
        ]))
        .block(panel(t("system"), app.settings.theme)),
        sections[0],
    );

    let track_area = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(sections[1]);

    frame.render_widget(panel(t("now_playing"), app.settings.theme), sections[1]);

    match app.playback.as_ref().and_then(|p| p.item.as_ref()) {
        Some(track) => {
            let artists = track
                .artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let position = app.position().min(track.duration_ms);
            let ratio = if track.duration_ms == 0 {
                0.0
            } else {
                position as f64 / track.duration_ms as f64
            };
            let state = if app.playback.as_ref().is_some_and(|p| p.is_playing) {
                t("playing")
            } else {
                t("paused")
            };

            frame.render_widget(
                Paragraph::new(track.name.as_str()).style(
                    Style::default()
                        .fg(foreground(app.settings.theme))
                        .add_modifier(Modifier::BOLD),
                ),
                track_area[0],
            );
            frame.render_widget(
                Paragraph::new(format!("{artists} / {}", track.album.name))
                    .style(Style::default().fg(muted)),
                track_area[1],
            );
            frame.render_widget(
                Gauge::default()
                    .ratio(ratio.clamp(0.0, 1.0))
                    .gauge_style(Style::default().fg(green))
                    .label(format!(
                        "{} / {}",
                        timestamp(position),
                        timestamp(track.duration_ms)
                    )),
                track_area[2],
            );
            frame.render_widget(
                Paragraph::new(state).style(Style::default().fg(green)),
                track_area[3],
            );
        }
        None => frame.render_widget(
            Paragraph::new(app.message.as_str()).style(Style::default().fg(muted)),
            track_area[0],
        ),
    }

    draw_lyrics(frame, app, sections[2], muted);

    let status = if app.connected {
        t("connected")
    } else {
        t("disconnected")
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" ● {status} "),
                Style::default().fg(if app.connected {
                    theme_color(app.settings.theme, 1)
                } else {
                    Color::Red
                }),
            ),
            Span::styled(t("footer_help"), Style::default().fg(muted)),
        ]))
        .block(panel(t("controls"), app.settings.theme)),
        sections[3],
    );
    if app.menu_open {
        draw_menu(frame, app);
    }
}

async fn run(terminal: &mut DefaultTerminal, settings_only: bool) -> Result<()> {
    let mut playback_worker = PlaybackWorker::new();
    let mut lyrics_loader = LyricsLoader::default();
    let mut app = App::new();
    app.menu_open = settings_only;
    if settings_only {
        app.update_message = "Esc: save and exit / Q: exit".to_owned();
    }
    if app.settings.font_size_pt != 0
        && let Err(error) = apply_kitty_font_size(app.settings.font_size_pt)
    {
        app.update_message = error;
    }
    let mut last_poll = Instant::now() - Duration::from_secs(31);
    let mut update_task: Option<tokio::task::JoinHandle<String>> = None;
    if app.settings.check_updates && !settings_only {
        update_task = Some(tokio::spawn(updates::check()));
    }

    loop {
        if update_task.as_ref().is_some_and(|task| task.is_finished())
            && let Some(task) = update_task.take()
        {
            app.update_message = task
                .await
                .unwrap_or_else(|_| "Update check failed".to_owned());
        }
        if !settings_only
            && last_poll.elapsed() >= Duration::from_secs(app.settings.refresh_seconds.clamp(1, 30))
            && playback_worker.request()
        {
            last_poll = Instant::now();
        }
        if let Some(sample) = playback_worker.try_result() {
            match sample.result {
                Ok(playback) => {
                    app.sync_clock(&playback, sample.requested_at, sample.received_at);
                    app.playback = playback;
                    app.connected = true;
                    app.message = "Nothing is currently playing".to_owned();
                    let track = app.playback.as_ref().and_then(|state| state.item.as_ref());
                    let key = track.map(|track| track.key());
                    if key != app.track_key {
                        app.track_key = key;
                        app.lyrics = None;
                        match track {
                            Some(track) => {
                                let cached = lyrics_loader.start(track);
                                app.lyrics_message = "Loading synchronized lyrics...".to_owned();
                                if let Some(lines) = cached {
                                    app.set_lyrics(Ok(lines));
                                }
                            }
                            None => {
                                lyrics_loader.cancel();
                                app.lyrics_message = "Waiting for a track...".to_owned();
                            }
                        }
                    }
                }
                Err(error) => {
                    app.connected = false;
                    app.message = error;
                }
            }
        }
        if let Some((key, result)) = lyrics_loader.take_ready().await
            && app.track_key.as_deref() == Some(key.as_str())
        {
            app.set_lyrics(result);
        }

        terminal.draw(|frame| draw(frame, &app))?;
        if event::poll(Duration::from_millis(
            1000 / app.settings.animation_fps.clamp(10, 60),
        ))? && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if app.menu_open {
                match key.code {
                    KeyCode::Esc => {
                        app.menu_open = false;
                        save_settings(&app.settings)?;
                        if settings_only {
                            break;
                        }
                    }
                    KeyCode::Char('q') if settings_only => break,
                    KeyCode::Up | KeyCode::Char('k') => {
                        app.menu_selection = app.menu_selection.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        app.menu_selection = (app.menu_selection + 1).min(15);
                    }
                    KeyCode::Left | KeyCode::Char('h') => edit_setting(&mut app, -1),
                    KeyCode::Right | KeyCode::Char('l') => edit_setting(&mut app, 1),
                    KeyCode::Enter => match app.menu_selection {
                        0..=11 => edit_setting(&mut app, 1),
                        12 => {
                            app.update_message = "Checking GitHub...".to_owned();
                            update_task = Some(tokio::spawn(updates::check()));
                        }
                        13 => save_settings(&app.settings)?,
                        14 => {
                            app.settings = Settings::default();
                            app.update_message = "Settings reset. Font size unchanged.".to_owned();
                        }
                        15 => {
                            app.menu_open = false;
                            save_settings(&app.settings)?;
                            if settings_only {
                                break;
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Esc => app.menu_open = true,
                    KeyCode::Char('r') => {
                        last_poll = Instant::now() - Duration::from_secs(31);
                        if app.lyrics.is_none()
                            && let Some(track) =
                                app.playback.as_ref().and_then(|state| state.item.as_ref())
                        {
                            let cached = lyrics_loader.retry(track);
                            app.lyrics_message = "Loading synchronized lyrics...".to_owned();
                            if let Some(lines) = cached {
                                app.set_lyrics(Ok(lines));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

async fn start(settings_only: bool) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, settings_only).await;
    ratatui::restore();
    result
}

async fn status() -> Result<()> {
    let playback = tokio::time::timeout(Duration::from_secs(15), async {
        let mut spotify = SpotifyClient::connect().await?;
        spotify.current_playback().await
    })
    .await
    .map_err(|_| anyhow::anyhow!("Spotify status request timed out"))??;
    println!("Spotify: Connected");
    match playback {
        Some(playback) => match playback.item {
            Some(track) => {
                println!("Track: {}", track.name);
                println!(
                    "Artist: {}",
                    track
                        .artists
                        .iter()
                        .map(|artist| artist.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                println!("Album: {}", track.album.name);
                println!(
                    "Playback: {}",
                    if playback.is_playing {
                        "Playing"
                    } else {
                        "Paused"
                    }
                );
                let position = playback
                    .progress_ms
                    .map(timestamp)
                    .unwrap_or_else(|| "Unknown".to_owned());
                println!("Position: {} / {}", position, timestamp(track.duration_ms));
            }
            None => println!("No music track is currently playing"),
        },
        None => println!("Nothing is currently playing"),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    match cli::parse(std::env::args().skip(1))? {
        cli::Command::Run => start(false).await?,
        cli::Command::Settings => start(true).await?,
        cli::Command::Login => spotify::login().await?,
        cli::Command::Logout => {
            println!(
                "{}",
                if spotify::logout()? {
                    "Logged out. Local Spotify credentials removed."
                } else {
                    "Already logged out."
                }
            );
        }
        cli::Command::Status => status().await?,
        cli::Command::Version => println!("lyricflow {}", env!("CARGO_PKG_VERSION")),
        cli::Command::Help => print!("{}", cli::HELP),
    }
    Ok(())
}
