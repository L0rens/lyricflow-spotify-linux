use crate::lyrics::LyricLine;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const MAX_APPROXIMATE_LINE_MS: u64 = 8_000;

fn rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Reset => None,
        Color::Black => Some((0, 0, 0)),
        Color::Red => Some((128, 0, 0)),
        Color::Green => Some((0, 128, 0)),
        Color::Yellow => Some((128, 128, 0)),
        Color::Blue => Some((0, 0, 128)),
        Color::Magenta => Some((128, 0, 128)),
        Color::Cyan => Some((0, 128, 128)),
        Color::Gray => Some((192, 192, 192)),
        Color::DarkGray => Some((128, 128, 128)),
        Color::LightRed => Some((255, 0, 0)),
        Color::LightGreen => Some((0, 255, 0)),
        Color::LightYellow => Some((255, 255, 0)),
        Color::LightBlue => Some((0, 0, 255)),
        Color::LightMagenta => Some((255, 0, 255)),
        Color::LightCyan => Some((0, 255, 255)),
        Color::White => Some((255, 255, 255)),
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(index) => match index {
            0..=15 => {
                const PALETTE: [(u8, u8, u8); 16] = [
                    (0, 0, 0),
                    (128, 0, 0),
                    (0, 128, 0),
                    (128, 128, 0),
                    (0, 0, 128),
                    (128, 0, 128),
                    (0, 128, 128),
                    (192, 192, 192),
                    (128, 128, 128),
                    (255, 0, 0),
                    (0, 255, 0),
                    (255, 255, 0),
                    (0, 0, 255),
                    (255, 0, 255),
                    (0, 255, 255),
                    (255, 255, 255),
                ];
                Some(PALETTE[usize::from(index)])
            }
            16..=231 => {
                let n = index - 16;
                let level = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
                Some((level(n / 36), level((n / 6) % 6), level(n % 6)))
            }
            _ => {
                let gray = 8 + (index - 232) * 10;
                Some((gray, gray, gray))
            }
        },
    }
}

pub fn blend(muted: Color, active: Color, fraction: f64) -> Color {
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction <= 0.0 {
        return muted;
    }
    if fraction >= 1.0 {
        return active;
    }
    let (Some((ar, ag, ab)), Some((br, bg, bb))) = (rgb(muted), rgb(active)) else {
        return muted;
    };
    let mix =
        |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * fraction).round() as u8;
    Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

fn fraction(now_ms: u64, start_ms: u64, end_ms: u64) -> f64 {
    if now_ms < start_ms {
        return 0.0;
    }
    if end_ms <= start_ms || now_ms >= end_ms {
        return 1.0;
    }
    (now_ms - start_ms) as f64 / (end_ms - start_ms) as f64
}

pub fn fill_end_ms(lines: &[LyricLine], current: usize, next_start_ms: u64) -> u64 {
    const FILL_REFERENCE_RADIUS: usize = 4;
    const MIN_FILL_REFERENCE_SAMPLES: usize = 3;
    const LONG_FILL_GAP_RATIO: f64 = 2.0;
    let Some(target) = lines.get(current) else {
        return next_start_ms;
    };
    let width = target.text.width();
    if !target.words.is_empty() || target.text.trim().is_empty() || width == 0 {
        return next_start_ms;
    }
    let first = current.saturating_sub(FILL_REFERENCE_RADIUS);
    let last = current
        .saturating_add(FILL_REFERENCE_RADIUS + 1)
        .min(lines.len());
    let mut rates = Vec::new();
    for (index, reference) in lines.iter().enumerate().take(last).skip(first) {
        if index == current || reference.text.trim().is_empty() {
            continue;
        }
        let reference_width = reference.text.width();
        let Some(next) = lines.get(index + 1) else {
            continue;
        };
        if reference_width == 0 || next.start_ms <= reference.start_ms {
            continue;
        }
        let end = reference
            .words
            .last()
            .and_then(|word| word.end_ms)
            .unwrap_or(next.start_ms);
        if end <= reference.start_ms || end > next.start_ms {
            continue;
        }
        rates.push((end - reference.start_ms) as f64 / reference_width as f64);
    }
    if rates.len() < MIN_FILL_REFERENCE_SAMPLES {
        return next_start_ms;
    }
    rates.sort_by(f64::total_cmp);
    let middle = rates.len() / 2;
    let median = if rates.len().is_multiple_of(2) {
        (rates[middle - 1] + rates[middle]) / 2.0
    } else {
        rates[middle]
    };
    let estimate = (median * width as f64)
        .round()
        .clamp(1.0, MAX_APPROXIMATE_LINE_MS as f64) as u64;
    if next_start_ms.saturating_sub(target.start_ms) as f64 > estimate as f64 * LONG_FILL_GAP_RATIO
    {
        target.start_ms.saturating_add(estimate).min(next_start_ms)
    } else {
        next_start_ms
    }
}

pub fn line(
    lyric: &LyricLine,
    now_ms: u64,
    next_start_ms: u64,
    muted: Color,
    active: Color,
    fill: bool,
) -> Line<'static> {
    if !fill {
        return Line::from(Span::styled(
            lyric.text.clone(),
            Style::default().fg(active).add_modifier(Modifier::BOLD),
        ));
    }
    let graphemes: Vec<&str> = lyric.text.graphemes(true).collect();
    let mut cells = Vec::with_capacity(graphemes.len() + 1);
    cells.push(0_usize);
    for grapheme in &graphemes {
        cells.push(cells.last().copied().unwrap_or_default() + grapheme.width());
    }
    let end = next_start_ms.min(lyric.start_ms.saturating_add(MAX_APPROXIMATE_LINE_MS));
    let approximate = fraction(now_ms, lyric.start_ms, end);
    let total = cells.last().copied().unwrap_or_default();
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (index, grapheme) in graphemes.into_iter().enumerate() {
        let (progress, start_cell, end_cell) = if lyric.words.is_empty() {
            (approximate, 0, total)
        } else if let Some((word_index, word)) = lyric
            .words
            .iter()
            .enumerate()
            .find(|(_, word)| word.start_grapheme <= index && index < word.end_grapheme)
        {
            let end_ms = word
                .end_ms
                .or_else(|| lyric.words.get(word_index + 1).map(|next| next.start_ms));
            let progress = end_ms.map_or_else(
                || if now_ms >= word.start_ms { 1.0 } else { 0.0 },
                |end_ms| fraction(now_ms, word.start_ms, end_ms),
            );
            let start = cells.get(word.start_grapheme).copied().unwrap_or_default();
            let end = cells.get(word.end_grapheme).copied().unwrap_or(total);
            (progress, start, end.max(start))
        } else {
            let progress = lyric
                .words
                .iter()
                .rev()
                .find(|word| word.end_grapheme <= index)
                .map_or_else(
                    || if now_ms >= lyric.start_ms { 1.0 } else { 0.0 },
                    |word| {
                        if now_ms >= word.end_ms.unwrap_or(word.start_ms) {
                            1.0
                        } else {
                            0.0
                        }
                    },
                );
            (progress, cells[index], cells[index + 1])
        };
        let width = cells[index + 1] - cells[index];
        let boundary = start_cell as f64 + progress * (end_cell - start_cell) as f64;
        let filled = if width == 0 {
            progress
        } else {
            ((boundary - cells[index] as f64) / width as f64).clamp(0.0, 1.0)
        };
        let style = Style::default()
            .fg(blend(muted, active, filled))
            .add_modifier(Modifier::BOLD);
        if let Some(previous) = spans.last_mut().filter(|previous| previous.style == style) {
            previous.content.to_mut().push_str(grapheme);
        } else {
            spans.push(Span::styled(grapheme.to_owned(), style));
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::WordTiming;

    const MUTED: Color = Color::Rgb(80, 80, 80);

    fn plain(text: &str) -> LyricLine {
        LyricLine {
            start_ms: 1_000,
            text: text.to_owned(),
            words: Vec::new(),
        }
    }

    fn rendered(lyric: &LyricLine, now: u64) -> Line<'static> {
        line(lyric, now, 5_000, MUTED, Color::White, true)
    }

    fn colors(rendered: &Line<'_>) -> Vec<Color> {
        rendered
            .spans
            .iter()
            .flat_map(|span| {
                span.content
                    .graphemes(true)
                    .map(move |_| span.style.fg.unwrap())
            })
            .collect()
    }

    #[test]
    fn named_highlights_do_not_fill_a_line_at_its_start() {
        for active in [Color::White, Color::Yellow, Color::Magenta] {
            let rendered = line(&plain("Test"), 1_000, 5_000, MUTED, active, true);
            assert_eq!(colors(&rendered), vec![MUTED; 4]);
        }
    }

    #[test]
    fn front_moves_left_to_right_and_responds_to_seek() {
        let lyric = plain("abcd");
        assert_eq!(
            colors(&rendered(&lyric, 2_000)),
            vec![Color::White, MUTED, MUTED, MUTED]
        );
        assert_eq!(
            colors(&rendered(&lyric, 3_000)),
            vec![Color::White, Color::White, MUTED, MUTED]
        );
        assert_eq!(colors(&rendered(&lyric, 1_000)), vec![MUTED; 4]);
        assert_eq!(colors(&rendered(&lyric, 5_000)), vec![Color::White; 4]);
    }

    #[test]
    fn frontier_blends_smoothly_without_changing_future_graphemes() {
        let rendered = rendered(&plain("abcd"), 1_500);
        assert_eq!(
            colors(&rendered),
            vec![Color::Rgb(168, 168, 168), MUTED, MUTED, MUTED]
        );
    }

    #[test]
    fn unchanged_music_time_produces_unchanged_frame() {
        let lyric = plain("Пауза");
        assert_eq!(rendered(&lyric, 2_345), rendered(&lyric, 2_345));
    }

    #[test]
    fn graphemes_are_intact_and_progress_uses_terminal_width() {
        let lyric = plain("界e\u{301}👩‍💻!");
        let rendered = rendered(&lyric, 3_000);
        let text: String = rendered
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, lyric.text);
        let colors = colors(&rendered);
        assert_eq!(colors.len(), 4);
        assert_eq!(colors[0], Color::White);
        assert_eq!(colors[1], Color::White);
        assert_eq!(colors[2], MUTED);
        assert_eq!(colors[3], MUTED);
    }

    #[test]
    fn enhanced_words_follow_source_intervals_including_final_end() {
        let lyric = LyricLine {
            start_ms: 1_000,
            text: "ABCD".to_owned(),
            words: vec![
                WordTiming {
                    start_ms: 1_000,
                    end_ms: Some(2_000),
                    start_grapheme: 0,
                    end_grapheme: 2,
                },
                WordTiming {
                    start_ms: 4_000,
                    end_ms: Some(6_000),
                    start_grapheme: 2,
                    end_grapheme: 4,
                },
            ],
        };
        assert_eq!(
            colors(&rendered(&lyric, 1_500)),
            vec![Color::White, MUTED, MUTED, MUTED]
        );
        assert_eq!(
            colors(&rendered(&lyric, 3_000)),
            vec![Color::White, Color::White, MUTED, MUTED]
        );
        assert_eq!(
            colors(&rendered(&lyric, 5_000)),
            vec![Color::White, Color::White, Color::White, MUTED]
        );
    }

    #[test]
    fn untimed_final_word_activates_at_its_start_without_invented_duration() {
        let lyric = LyricLine {
            start_ms: 1_000,
            text: "AB".to_owned(),
            words: vec![WordTiming {
                start_ms: 2_000,
                end_ms: None,
                start_grapheme: 0,
                end_grapheme: 2,
            }],
        };
        assert_eq!(colors(&rendered(&lyric, 1_999)), vec![MUTED, MUTED]);
        assert_eq!(
            colors(&rendered(&lyric, 2_000)),
            vec![Color::White, Color::White]
        );
    }

    #[test]
    fn instrumental_gap_does_not_stretch_fill_indefinitely() {
        let rendered = line(&plain("abcd"), 9_000, 60_000, MUTED, Color::White, true);
        assert_eq!(colors(&rendered), vec![Color::White; 4]);
    }

    #[test]
    fn disabling_fill_keeps_the_whole_active_line_visible() {
        let rendered = line(&plain("abcd"), 0, 5_000, MUTED, Color::White, false);
        assert_eq!(colors(&rendered), vec![Color::White; 4]);
    }

    #[test]
    fn reset_and_indexed_colors_preserve_unfilled_text() {
        assert_eq!(blend(Color::Reset, Color::White, 0.5), Color::Reset);
        assert_eq!(
            blend(Color::Indexed(232), Color::White, 0.0),
            Color::Indexed(232)
        );
        assert_eq!(
            blend(Color::Indexed(232), Color::White, 0.5),
            Color::Rgb(132, 132, 132)
        );
    }

    fn pause_context() -> Vec<LyricLine> {
        [1_000, 5_000, 9_000, 13_000, 25_000, 29_000, 33_000]
            .into_iter()
            .enumerate()
            .map(|(index, start_ms)| LyricLine {
                start_ms,
                text: if index == 3 {
                    "Whatever it takes".to_owned()
                } else {
                    "x".repeat(32)
                },
                words: Vec::new(),
            })
            .collect()
    }

    #[test]
    fn short_phrase_finishes_before_a_long_instrumental_gap_and_holds() {
        let lines = pause_context();
        let end = fill_end_ms(&lines, 3, 25_000);
        assert_eq!(end, 15_125);
        let halfway = line(&lines[3], 14_000, end, MUTED, Color::White, true);
        assert!(colors(&halfway).contains(&MUTED));
        let completed = line(&lines[3], end, end, MUTED, Color::White, true);
        assert!(
            colors(&completed)
                .iter()
                .all(|color| *color == Color::White)
        );
        assert_eq!(
            completed,
            line(&lines[3], 24_000, end, MUTED, Color::White, true)
        );
    }

    #[test]
    fn ordinary_contiguous_line_timing_is_preserved() {
        let mut lines = pause_context();
        lines[4].start_ms = 17_000;
        lines[5].start_ms = 21_000;
        lines[6].start_ms = 25_000;
        assert_eq!(fill_end_ms(&lines, 3, 17_000), 17_000);
    }

    #[test]
    fn enhanced_target_keeps_real_source_timing_during_a_long_hold() {
        let mut lines = pause_context();
        lines[3].words.push(WordTiming {
            start_ms: 13_000,
            end_ms: Some(23_000),
            start_grapheme: 0,
            end_grapheme: 17,
        });
        let end = fill_end_ms(&lines, 3, 25_000);
        assert_eq!(end, 25_000);
        let during_hold = line(&lines[3], 20_000, end, MUTED, Color::White, true);
        assert!(colors(&during_hold).contains(&MUTED));
    }

    #[test]
    fn insufficient_context_keeps_existing_fallback() {
        let mut lines = pause_context();
        lines.truncate(3);
        assert_eq!(fill_end_ms(&lines, 2, 25_000), 25_000);
        assert_eq!(fill_end_ms(&[], 0, 25_000), 25_000);
    }

    #[test]
    fn empty_zero_width_and_duplicate_intervals_do_not_supply_reference_rates() {
        let mut lines = pause_context();
        lines[0].text.clear();
        lines[1].text = "\u{200b}".to_owned();
        lines[2].start_ms = lines[3].start_ms;
        assert_eq!(fill_end_ms(&lines, 3, 25_000), 25_000);
    }

    #[test]
    fn enhanced_reference_uses_its_explicit_vocal_end() {
        let mut lines = pause_context();
        for reference in lines.iter_mut().take(3) {
            reference.words.push(WordTiming {
                start_ms: reference.start_ms,
                end_ms: Some(reference.start_ms + 2_000),
                start_grapheme: 0,
                end_grapheme: 32,
            });
        }
        assert_eq!(fill_end_ms(&lines, 3, 25_000), 14_063);
    }
}
