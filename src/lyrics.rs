use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug)]
pub struct LyricLine {
    pub start_ms: u64,
    pub text: String,
    pub words: Vec<WordTiming>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordTiming {
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub start_grapheme: usize,
    pub end_grapheme: usize,
}

#[derive(Deserialize)]
struct LyricsResponse {
    #[serde(rename = "syncedLyrics")]
    synced_lyrics: Option<String>,
    instrumental: bool,
}

pub async fn fetch(
    track: &str,
    artist: &str,
    album: &str,
    duration_ms: u64,
) -> Result<Option<Vec<LyricLine>>> {
    let client = reqwest::Client::builder()
        .user_agent(concat!(
            "lyricflow/",
            env!("CARGO_PKG_VERSION"),
            " (terminal lyrics client)"
        ))
        .timeout(Duration::from_secs(12))
        .build()?;
    let duration = (duration_ms / 1000).to_string();
    let response = client
        .get("https://lrclib.net/api/get")
        .query(&[
            ("track_name", track),
            ("artist_name", artist),
            ("album_name", album),
            ("duration", duration.as_str()),
        ])
        .send()
        .await?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }

    let data: LyricsResponse = response
        .error_for_status()
        .context("LRCLIB request failed")?
        .json()
        .await?;

    if data.instrumental {
        return Ok(None);
    }

    Ok(data
        .synced_lyrics
        .map(|lyrics| within_duration(parse_lrc(&lyrics), duration_ms))
        .filter(|lines| lines.iter().any(|line| !line.text.is_empty())))
}

fn within_duration(mut lines: Vec<LyricLine>, duration_ms: u64) -> Vec<LyricLine> {
    lines.retain(|line| line.start_ms <= duration_ms);
    for line in &mut lines {
        if line.words.iter().any(|word| {
            word.start_ms > duration_ms || word.end_ms.is_some_and(|end| end > duration_ms)
        }) {
            line.words.clear();
        }
    }
    lines
}

fn timestamp(source: &str) -> Option<u64> {
    let (minutes, seconds) = source.split_once(':')?;
    if minutes.is_empty() || !minutes.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let (seconds, fraction) = match seconds.split_once('.') {
        Some((seconds, fraction)) => {
            if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            (seconds, fraction)
        }
        None => (seconds, ""),
    };
    if seconds.is_empty() || !seconds.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let seconds = seconds.parse::<u64>().ok()?;
    if seconds >= 60 {
        return None;
    }
    let milliseconds = fraction
        .bytes()
        .take(3)
        .chain(std::iter::repeat(b'0'))
        .take(3)
        .fold(0u64, |value, digit| value * 10 + u64::from(digit - b'0'));
    minutes
        .parse::<u64>()
        .ok()?
        .checked_mul(60_000)?
        .checked_add(seconds * 1_000)?
        .checked_add(milliseconds)
}

fn offset_time(time: u64, offset: i64) -> u64 {
    if offset >= 0 {
        time.saturating_sub(offset as u64)
    } else {
        time.saturating_add(offset.unsigned_abs())
    }
}

fn enhanced_text(source: &str, line_start: u64) -> (String, Vec<WordTiming>) {
    let mut output = String::new();
    let mut markers = Vec::new();
    let mut invalid = false;
    let mut rest = source;
    while let Some(open) = rest.find('<') {
        output.push_str(&rest[..open]);
        let Some(close) = rest[open + 1..].find('>') else {
            let candidate = &rest[open + 1..];
            let token_end = candidate
                .find(char::is_whitespace)
                .unwrap_or(candidate.len());
            let token = &candidate[..token_end];
            let timestamp_like = token.split_once(':').is_some_and(|(minutes, _)| {
                !minutes.is_empty() && minutes.bytes().all(|byte| byte.is_ascii_digit())
            });
            if timestamp_like {
                invalid = true;
                output.push_str(&candidate[token_end..]);
            } else {
                output.push_str(&rest[open..]);
            }
            rest = "";
            break;
        };
        let close = open + 1 + close;
        let tag = &rest[open + 1..close];
        if let Some(time) = timestamp(tag) {
            markers.push((output.len(), time));
        } else if tag.contains(':') {
            invalid = true;
        } else {
            output.push_str(&rest[open..=close]);
        }
        rest = &rest[close + 1..];
    }
    output.push_str(rest);
    let leading = output.len() - output.trim_start().len();
    let trailing = output.trim_end().len();
    let text = output.trim().to_owned();
    if invalid || markers.is_empty() || text.is_empty() {
        return (text, Vec::new());
    }
    let boundaries: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut positions = Vec::new();
    for (byte, time) in markers {
        let byte = byte.clamp(leading, trailing.max(leading)) - leading;
        let Ok(grapheme) = boundaries.binary_search(&byte) else {
            return (text, Vec::new());
        };
        if time < line_start || positions.last().is_some_and(|&(_, last)| time < last) {
            return (text, Vec::new());
        }
        positions.push((grapheme, time));
    }
    if positions.first().is_some_and(|&(index, _)| index != 0) {
        return (text, Vec::new());
    }
    let mut words = Vec::new();
    for (index, &(start_grapheme, start_ms)) in positions.iter().enumerate() {
        let next = positions.get(index + 1);
        let end_grapheme = next.map_or(boundaries.len() - 1, |&(position, _)| position);
        if end_grapheme > start_grapheme {
            words.push(WordTiming {
                start_ms,
                end_ms: next.map(|&(_, time)| time),
                start_grapheme,
                end_grapheme,
            });
        }
    }
    (text, words)
}

fn parse_lrc(source: &str) -> Vec<LyricLine> {
    let offset = source
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("[offset:")?
                .strip_suffix(']')?
                .trim()
                .parse::<i64>()
                .ok()
        })
        .next_back()
        .unwrap_or(0);
    let mut lines = Vec::new();
    for raw in source.lines() {
        let mut rest = raw.trim_start_matches('\u{feff}').trim_start();
        let mut times = Vec::new();
        while let Some(tag) = rest.strip_prefix('[') {
            let Some((value, after)) = tag.split_once(']') else {
                break;
            };
            let Some(time) = timestamp(value) else {
                break;
            };
            times.push(time);
            rest = after;
        }
        let Some(&first_time) = times.first() else {
            continue;
        };
        let (text, words) = enhanced_text(rest, first_time);
        for time in times {
            let shift = time as i128 - first_time as i128;
            let shift_word =
                |word_time: u64| -> Option<u64> { u64::try_from(word_time as i128 + shift).ok() };
            let shifted: Option<Vec<WordTiming>> = words
                .iter()
                .map(|word| {
                    Some(WordTiming {
                        start_ms: offset_time(shift_word(word.start_ms)?, offset),
                        end_ms: match word.end_ms {
                            Some(end) => Some(offset_time(shift_word(end)?, offset)),
                            None => None,
                        },
                        start_grapheme: word.start_grapheme,
                        end_grapheme: word.end_grapheme,
                    })
                })
                .collect();
            lines.push(LyricLine {
                start_ms: offset_time(time, offset),
                text: text.clone(),
                words: shifted.unwrap_or_default(),
            });
        }
    }
    lines.sort_by_key(|line| line.start_ms);
    for index in 0..lines.len() {
        let next_start = lines[index + 1..]
            .iter()
            .find(|line| line.start_ms > lines[index].start_ms)
            .map(|line| line.start_ms);
        let invalid = next_start.is_some_and(|end| {
            lines[index]
                .words
                .iter()
                .any(|word| word.start_ms >= end || word.end_ms.is_some_and(|time| time > end))
        });
        if invalid {
            lines[index].words.clear();
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{parse_lrc, timestamp, within_duration};

    #[test]
    fn parses_integer_fractional_seconds_without_float_rounding() {
        assert_eq!(timestamp("00:01.1"), Some(1_100));
        assert_eq!(timestamp("00:01.12"), Some(1_120));
        assert_eq!(timestamp("00:01.1239"), Some(1_123));
        assert_eq!(timestamp("02:03"), Some(123_000));
        let lines = parse_lrc("[00:12.50] First line\n[00:14.00] Second line");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].start_ms, 12_500);
        assert_eq!(lines[1].text, "Second line");
    }

    #[test]
    fn rejects_malformed_and_overflowing_timestamps() {
        for source in [
            "00:60",
            "00:-1",
            "00:NaN",
            "00:inf",
            "00:1.",
            "00:1.2x",
            "-1:00",
            "18446744073709551615:00",
            "00:18446744073709551615",
            "1:2:3",
        ] {
            assert_eq!(timestamp(source), None, "{source}");
        }
        assert!(parse_lrc("[ar:Artist]\n[00:NaN]bad\n[99:no]bad").is_empty());
    }

    #[test]
    fn repeated_line_timestamps_and_empty_instrumental_markers_survive() {
        let lines = parse_lrc("[00:02][00:08] Again\n[00:04]\n[00:12] End");
        assert_eq!(
            lines.iter().map(|line| line.start_ms).collect::<Vec<_>>(),
            vec![2_000, 4_000, 8_000, 12_000]
        );
        assert!(lines[1].text.is_empty());
        assert_eq!(lines[2].text, "Again");
    }

    #[test]
    fn positive_offset_advances_lyrics_and_clamps_at_zero() {
        let lines = parse_lrc("[00:00.1] First\n[00:02] Second\n[offset:+500]");
        assert_eq!(lines[0].start_ms, 0);
        assert_eq!(lines[1].start_ms, 1_500);
        let lines = parse_lrc("[offset:-250]\n[00:01] Later");
        assert_eq!(lines[0].start_ms, 1_250);
    }

    #[test]
    fn enhanced_lyrics_preserve_unicode_graphemes_and_closing_end_tag() {
        let lines = parse_lrc("[00:10]<00:10>Привет <00:11>е\u{301}👩‍👩‍👧‍👦<00:12>");
        let line = &lines[0];
        assert_eq!(line.text, "Привет е\u{301}👩‍👩‍👧‍👦");
        assert_eq!(line.words.len(), 2);
        assert_eq!(line.words[0].start_grapheme, 0);
        assert_eq!(line.words[0].end_grapheme, 7);
        assert_eq!(line.words[0].end_ms, Some(11_000));
        assert_eq!(line.words[1].start_grapheme, 7);
        assert_eq!(line.words[1].end_grapheme, 9);
        assert_eq!(line.words[1].end_ms, Some(12_000));
    }

    #[test]
    fn enhanced_last_word_without_end_remains_explicitly_unknown() {
        let lines = parse_lrc("[00:10]<00:10>One <00:11>two");
        assert_eq!(lines[0].words[1].end_ms, None);
    }

    #[test]
    fn enhanced_repeated_lines_shift_word_times() {
        let lines = parse_lrc("[00:10][00:20]<00:10>One <00:11>two<00:12>");
        assert_eq!(lines[1].words[0].start_ms, 20_000);
        assert_eq!(lines[1].words[1].end_ms, Some(22_000));
    }

    #[test]
    fn invalid_word_order_and_bounds_fall_back_to_clean_plain_text() {
        for source in [
            "[00:10]<00:11>One <00:10>two",
            "[00:10]<00:09>One <00:11>two",
            "[00:10]<00:10>One <00:60>two",
            "[00:10]One <00:11>two",
            "[00:10]<00:10>One <00:12>two\n[00:11]Next",
        ] {
            let lines = parse_lrc(source);
            assert_eq!(lines[0].text, "One two");
            assert!(lines[0].words.is_empty(), "{source}");
        }
    }

    #[test]
    fn timing_tag_cannot_split_a_combining_grapheme() {
        let lines = parse_lrc("[00:10]<00:10>e<00:11>\u{301}");
        assert_eq!(lines[0].text, "e\u{301}");
        assert!(lines[0].words.is_empty());
    }

    #[test]
    fn offset_applies_to_enhanced_timestamps_as_well_as_line_time() {
        let lines = parse_lrc("[offset:500]\n[00:10]<00:10>One<00:11>");
        assert_eq!(lines[0].start_ms, 9_500);
        assert_eq!(lines[0].words[0].start_ms, 9_500);
        assert_eq!(lines[0].words[0].end_ms, Some(10_500));
    }

    #[test]
    fn final_word_times_cannot_extend_past_the_track_duration() {
        for source in ["[00:10]<00:10>One<00:31>", "[00:10]<00:31>One"] {
            let lines = within_duration(parse_lrc(source), 30_000);
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0].text, "One");
            assert!(lines[0].words.is_empty(), "{source}");
        }
        let lines = within_duration(parse_lrc("[00:10]<00:10>One<00:30>"), 30_000);
        assert_eq!(lines[0].words[0].end_ms, Some(30_000));
    }

    #[test]
    fn lines_beyond_the_track_duration_are_omitted() {
        let lines = within_duration(parse_lrc("[00:10]One\n[00:31]Too late"), 30_000);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "One");
        assert!(within_duration(parse_lrc("[00:31]Too late"), 30_000).is_empty());
    }

    #[test]
    fn unclosed_timestamp_tags_fall_back_to_clean_text() {
        for source in [
            "[00:10]<00:10>One <00:11",
            "[00:10]One <00:11",
            "[00:10]One <00:xx",
        ] {
            let lines = parse_lrc(source);
            assert_eq!(lines[0].text, "One");
            assert!(lines[0].words.is_empty());
        }
        let lines = parse_lrc("[00:10]One <00:11 two");
        assert_eq!(lines[0].text, "One  two");
    }
}
