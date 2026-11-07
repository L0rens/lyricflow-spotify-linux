## lyricflow 0.1.1

Terminal karaoke for Spotify on Linux, with diagnostics and an offline preview.

- Run `lyricflow doctor` to check configuration and API access without printing tokens.
- Use `lyricflow doctor --offline` for local diagnostics only.
- Try `lyricflow demo` without a Spotify account, or load `lyricflow demo --file example.lrc`.
- Demo controls: Space pauses, Left/Right seek, R restarts, Q quits.
- Lyrics are cached locally; fallback search checks title, artist and duration.
- Includes the layout, highlighting, XDG storage and Spotify recovery fixes listed in CHANGELOG.md.

### Linux binary

Download the x86_64 archive and SHA256SUMS, then verify the archive before installing:

```sh
sha256sum --check SHA256SUMS
tar -xzf lyricflow-v0.1.1-x86_64-unknown-linux-gnu.tar.gz
install -Dm755 lyricflow-v0.1.1-x86_64-unknown-linux-gnu/lyricflow "$HOME/.local/bin/lyricflow"
```

Ensure `~/.local/bin` is on PATH. The hosted Linux build uses Ubuntu 22.04 and
requires glibc 2.35 or newer; this includes current Arch-based desktops. Settings
and Spotify authorization remain in the XDG configuration directory.

Ordinary LRC fill is approximate. Exact word timing is used only when supplied
by the lyric source. Demo mode uses original sample text and needs no API access.
