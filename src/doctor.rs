use crate::{spotify::SpotifyClient, spotify_error::SpotifyError, storage};
use std::{fs, path::Path, process::Stdio, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ok,
    Warn,
    Skip,
    Error,
}

struct Check {
    name: &'static str,
    state: State,
    detail: String,
}

impl Check {
    fn new(name: &'static str, state: State, detail: impl Into<String>) -> Self {
        Self {
            name,
            state,
            detail: detail.into(),
        }
    }
}

pub struct Report {
    checks: Vec<Check>,
}

impl Report {
    pub fn healthy(&self) -> bool {
        self.checks.iter().all(|check| check.state != State::Error)
    }

    pub fn render(&self) -> String {
        let mut output = format!("lyricflow {} diagnostics\n", env!("CARGO_PKG_VERSION"));
        for check in &self.checks {
            let state = match check.state {
                State::Ok => "OK",
                State::Warn => "WARN",
                State::Skip => "SKIP",
                State::Error => "ERROR",
            };
            output.push_str(&format!(
                "{:<20} {:<6} {}\n",
                check.name, state, check.detail
            ));
        }
        output
    }
}

fn inspect_directory(path: &Path) -> Check {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Check::new("Config directory", State::Ok, "Present"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Check::new(
            "Config directory",
            State::Warn,
            "Missing; created when saving settings or logging in",
        ),
        _ => Check::new(
            "Config directory",
            State::Error,
            "Unavailable or not a directory",
        ),
    }
}

fn inspect_tokens(path: &Path) -> Check {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Check::new(
                        "Spotify token file",
                        State::Warn,
                        "Present; group/other permissions should be removed (chmod 600)",
                    );
                }
            }
            Check::new(
                "Spotify token file",
                State::Ok,
                "Present; validity requires an online check",
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Check::new(
            "Spotify token file",
            State::Error,
            "Missing; run `lyricflow login`",
        ),
        _ => Check::new(
            "Spotify token file",
            State::Error,
            "Unavailable or not a regular file",
        ),
    }
}

async fn spotify_check() -> Check {
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let mut client = SpotifyClient::connect().await?;
        client.current_playback().await
    })
    .await;
    match result {
        Ok(Ok(_)) => Check::new(
            "Spotify API",
            State::Ok,
            "Authenticated; playback endpoint responded",
        ),
        Ok(Err(error)) => Check::new(
            "Spotify API",
            State::Error,
            error
                .downcast_ref::<SpotifyError>()
                .map_or("Spotify check failed".to_owned(), ToString::to_string),
        ),
        Err(_) => Check::new("Spotify API", State::Error, "Request timed out"),
    }
}

async fn lrclib_check() -> Check {
    let result = async {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .user_agent(concat!("lyricflow/", env!("CARGO_PKG_VERSION")))
            .build()?
            .get("https://lrclib.net/api/search")
            .query(&[("q", "lyricflow")])
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<serde_json::Value>>()
            .await
    }
    .await;
    match result {
        Ok(_) => Check::new("LRCLIB", State::Ok, "Search endpoint responded"),
        Err(_) => Check::new(
            "LRCLIB",
            State::Error,
            "API unavailable or unsupported response",
        ),
    }
}

async fn kitty_check(offline: bool) -> Check {
    if std::env::var_os("KITTY_WINDOW_ID").is_none() {
        return Check::new(
            "Kitty controls",
            State::Skip,
            "Not running in Kitty; optional",
        );
    }
    if offline {
        return Check::new(
            "Kitty controls",
            State::Skip,
            "Not probed in offline mode; optional",
        );
    }
    let mut command = tokio::process::Command::new("kitty");
    command
        .args(["@", "ls"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(2), command.status()).await {
        Ok(Ok(status)) if status.success() => {
            Check::new("Kitty controls", State::Ok, "Remote control available")
        }
        _ => Check::new(
            "Kitty controls",
            State::Warn,
            "Remote control unavailable; optional",
        ),
    }
}

pub async fn run(offline: bool) -> Report {
    let mut checks = Vec::new();
    let token_present = match storage::config_dir() {
        Ok(directory) => {
            checks.push(inspect_directory(&directory));
            let token = inspect_tokens(&directory.join("tokens.json"));
            let present = token.state != State::Error;
            checks.push(token);
            present
        }
        Err(_) => {
            checks.push(Check::new(
                "Config directory",
                State::Error,
                "Set an absolute XDG_CONFIG_HOME or HOME",
            ));
            false
        }
    };
    let client_id_present = std::env::var("LYRICFLOW_CLIENT_ID")
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    checks.push(Check::new(
        "Spotify client ID",
        if client_id_present {
            State::Ok
        } else {
            State::Error
        },
        if client_id_present {
            "Configured (value hidden)"
        } else {
            "Not configured; set LYRICFLOW_CLIENT_ID"
        },
    ));
    if offline {
        checks.push(Check::new("Spotify API", State::Skip, "Offline mode"));
        checks.push(Check::new("LRCLIB", State::Skip, "Offline mode"));
    } else {
        let (spotify, lrclib) = tokio::join!(
            async {
                if token_present && client_id_present {
                    spotify_check().await
                } else {
                    Check::new(
                        "Spotify API",
                        State::Skip,
                        "Configure client ID and login first",
                    )
                }
            },
            lrclib_check()
        );
        checks.push(spotify);
        checks.push(lrclib);
    }
    checks.push(match crossterm::terminal::size() {
        Ok((width, height)) => Check::new(
            "Terminal size",
            if width >= 30 && height >= 10 {
                State::Ok
            } else {
                State::Warn
            },
            format!("{width}x{height}; minimum 30x10"),
        ),
        Err(_) => Check::new("Terminal size", State::Warn, "Not an interactive terminal"),
    });
    checks.push(kitty_check(offline).await);
    Report { checks }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_checks_do_not_read_or_print_credential_contents() {
        let directory =
            std::env::temp_dir().join(format!("lyricflow-doctor-{:016x}", rand::random::<u64>()));
        fs::create_dir(&directory).unwrap();
        let token = directory.join("tokens.json");
        fs::write(&token, "private-marker-not-for-output").unwrap();
        let report = Report {
            checks: vec![inspect_directory(&directory), inspect_tokens(&token)],
        };
        assert!(report.healthy());
        assert!(!report.render().contains("private-marker"));
        fs::remove_file(&token).unwrap();
        assert_eq!(inspect_tokens(&token).state, State::Error);
        fs::remove_dir(&directory).unwrap();
    }
    #[test]
    fn optional_warnings_do_not_hide_required_failures() {
        let report = Report {
            checks: vec![Check::new("Kitty", State::Warn, "Unavailable")],
        };
        assert!(report.healthy());
        let report = Report {
            checks: vec![Check::new("Spotify", State::Error, "Not connected")],
        };
        assert!(!report.healthy());
        assert!(report.render().contains("ERROR"));
    }
}
