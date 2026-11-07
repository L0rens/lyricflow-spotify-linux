# Changelog

## 0.1.1

- Add `lyricflow doctor` with private local checks and optional online diagnostics.
- Add `lyricflow doctor --offline` without network probes.
- Add offline demo playback, custom LRC files, pause, seek and restart controls.
- Cache synchronized lyrics on disk with version, metadata and age validation.
- Search LRCLIB after an exact lookup misses, rejecting mismatched or ambiguous recordings.
- Separate elapsed time, progress bar and duration; adapt layout for small terminals.
- Respect the selected highlight color in every theme.
- Handle Spotify authorization failures, access denial, timeouts and rate limits.
- Respect XDG configuration paths and save configuration atomically.
- Add default update checks, continuous integration and Linux release packaging.
- Replace the README screenshot with an animated preview.

## 0.1.0

- Initial public Rust terminal karaoke application with Spotify PKCE login.
- Progressive Unicode-aware highlighting and standard/Enhanced LRC parsing.
- Configurable themes, languages, lyric offset and terminal settings.
