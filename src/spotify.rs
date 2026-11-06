use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

const REDIRECT_URI: &str = "http://127.0.0.1:8888/callback";

#[derive(Serialize, Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

fn token_path() -> Result<PathBuf> {
    let home = env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("lyricflow")
        .join("tokens.json"))
}

pub fn logout() -> Result<bool> {
    remove_tokens(&token_path()?)
}

fn remove_tokens(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("Could not remove local Spotify credentials"),
    }
}

pub async fn login() -> Result<()> {
    let client_id = env::var("LYRICFLOW_CLIENT_ID").context("LYRICFLOW_CLIENT_ID is not set")?;

    let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>());

    let listener = TcpListener::bind("127.0.0.1:8888")
        .context("Could not start the authentication callback server")?;

    listener.set_nonblocking(true)?;

    let mut auth_url = Url::parse("https://accounts.spotify.com/authorize")?;

    auth_url
        .query_pairs_mut()
        .append_pair("client_id", &client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", &challenge)
        .append_pair("state", &state)
        .append_pair(
            "scope",
            "user-read-currently-playing user-read-playback-state",
        );

    println!("Opening Spotify authorization...");
    println!("If the browser does not open, visit:\n{auth_url}");

    open::that(auth_url.as_str()).ok();

    let code = tokio::task::spawn_blocking(move || -> Result<String> {
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > Duration::from_secs(180) {
                bail!("Authentication timed out");
            }

            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(error) => return Err(error.into()),
            };

            stream.set_read_timeout(Some(Duration::from_secs(5)))?;

            let mut buffer = [0u8; 8192];
            let size = stream.read(&mut buffer)?;
            let request = String::from_utf8_lossy(&buffer[..size]);

            let Some(target) = request.split_whitespace().nth(1) else {
                continue;
            };

            let callback = Url::parse(&format!("http://127.0.0.1:8888{target}"))?;

            if callback.path() != "/callback" {
                continue;
            }

            let parameters: std::collections::HashMap<String, String> =
                callback.query_pairs().into_owned().collect();

            let valid_state = parameters.get("state") == Some(&state);

            let result = if !valid_state {
                Err(anyhow::anyhow!("Invalid OAuth state"))
            } else if let Some(error) = parameters.get("error") {
                Err(anyhow::anyhow!("Spotify authorization failed: {error}"))
            } else {
                parameters
                    .get("code")
                    .cloned()
                    .context("Authorization code is missing")
            };

            let message = if result.is_ok() {
                "Authentication received. You can close this tab."
            } else {
                "Authentication failed. Return to the terminal."
            };

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                message.len(),
                message
            );

            stream.write_all(response.as_bytes())?;
            return result;
        }
    })
    .await??;

    let client = reqwest::Client::new();

    let response = client
        .post("https://accounts.spotify.com/api/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await?
        .error_for_status()
        .context("Spotify rejected the token request")?;

    let token: TokenResponse = response.json().await?;

    let tokens = Tokens {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_in: token.expires_in,
    };

    let path = token_path()?;

    fs::create_dir_all(path.parent().context("Invalid token path")?)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;

        serde_json::to_writer_pretty(&mut file, &tokens)?;
    }

    #[cfg(not(unix))]
    fs::write(&path, serde_json::to_vec_pretty(&tokens)?)?;

    println!("Successfully connected to Spotify.");
    Ok(())
}

#[derive(Debug, Deserialize, Clone)]
pub struct Playback {
    pub is_playing: bool,
    pub progress_ms: Option<u64>,
    pub item: Option<Track>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Track {
    pub id: Option<String>,
    pub uri: Option<String>,
    pub name: String,
    pub duration_ms: u64,
    pub artists: Vec<Artist>,
    pub album: Album,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Artist {
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Album {
    pub name: String,
}

impl Track {
    pub fn key(&self) -> String {
        if let Some(id) = self.id.as_deref().filter(|id| !id.is_empty()) {
            return format!("id:{id}");
        }
        if let Some(uri) = self.uri.as_deref().filter(|uri| !uri.is_empty()) {
            return format!("uri:{uri}");
        }
        let artists: Vec<_> = self
            .artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect();
        format!(
            "metadata:{:?}",
            (&self.name, artists, &self.album.name, self.duration_ms)
        )
    }
}

pub struct SpotifyClient {
    client: reqwest::Client,
    client_id: String,
    tokens: Tokens,
    expires_at: std::time::Instant,
}

impl SpotifyClient {
    pub async fn connect() -> Result<Self> {
        let client_id =
            env::var("LYRICFLOW_CLIENT_ID").context("LYRICFLOW_CLIENT_ID is not set")?;
        let data = fs::read(token_path()?)
            .context("Spotify is not connected. Run `lyricflow login` first")?;
        let tokens: Tokens = serde_json::from_slice(&data)?;
        let mut spotify = Self {
            client: reqwest::Client::new(),
            client_id,
            tokens,
            expires_at: std::time::Instant::now(),
        };
        spotify.refresh().await?;
        Ok(spotify)
    }

    async fn refresh(&mut self) -> Result<()> {
        let refresh_token = self
            .tokens
            .refresh_token
            .as_deref()
            .context("Missing refresh token. Run `lyricflow login` again")?
            .to_owned();
        let response = self
            .client
            .post("https://accounts.spotify.com/api/token")
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
                ("client_id", self.client_id.as_str()),
            ])
            .send()
            .await?
            .error_for_status()
            .context("Failed to refresh Spotify authorization")?;
        let token: TokenResponse = response.json().await?;
        self.tokens.access_token = token.access_token;
        self.tokens.refresh_token = Some(token.refresh_token.unwrap_or(refresh_token));
        self.tokens.expires_in = token.expires_in;
        self.expires_at =
            std::time::Instant::now() + Duration::from_secs(token.expires_in.saturating_sub(60));

        let path = token_path()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)?;
            serde_json::to_writer_pretty(&mut file, &self.tokens)?;
        }
        #[cfg(not(unix))]
        fs::write(&path, serde_json::to_vec_pretty(&self.tokens)?)?;
        Ok(())
    }

    pub async fn current_playback(&mut self) -> Result<Option<Playback>> {
        self.sample_playback()
            .await
            .map(|(_, _, playback)| playback)
    }

    pub async fn sample_playback(
        &mut self,
    ) -> Result<(std::time::Instant, std::time::Instant, Option<Playback>)> {
        if std::time::Instant::now() >= self.expires_at {
            self.refresh().await?;
        }
        let requested_at = std::time::Instant::now();
        let response = self
            .client
            .get("https://api.spotify.com/v1/me/player/currently-playing")
            .bearer_auth(&self.tokens.access_token)
            .send()
            .await?
            .error_for_status()
            .context("Failed to fetch Spotify playback")?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok((requested_at, std::time::Instant::now(), None));
        }
        let playback = response.json::<Playback>().await?;
        Ok((requested_at, std::time::Instant::now(), Some(playback)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
            loop {
                let path = env::temp_dir().join(format!(
                    "lyricflow-logout-{}-{}",
                    std::process::id(),
                    NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("Could not create test directory: {error}"),
                }
            }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn logout_removes_only_credentials_and_is_idempotent() {
        let directory = TestDirectory::new();
        let credentials = directory.0.join("tokens.json");
        let settings = directory.0.join("settings.json");
        let other_file = directory.0.join("another-file.txt");
        let settings_contents = br#"{"lyrics_offset_ms":250,"theme":2}"#;
        fs::write(&credentials, "fake credentials for the logout test").unwrap();
        fs::write(&settings, settings_contents).unwrap();
        fs::write(&other_file, "preserve this file").unwrap();

        assert!(remove_tokens(&credentials).unwrap());
        assert!(!credentials.exists());
        assert_eq!(fs::read(&settings).unwrap(), settings_contents);
        assert_eq!(
            fs::read_to_string(&other_file).unwrap(),
            "preserve this file"
        );
        assert!(!remove_tokens(&credentials).unwrap());
        assert!(directory.0.is_dir());
    }

    #[test]
    fn missing_credentials_are_already_logged_out() {
        let directory = TestDirectory::new();
        assert!(!remove_tokens(&directory.0.join("tokens.json")).unwrap());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }

    #[test]
    fn logout_reports_other_filesystem_errors() {
        let directory = TestDirectory::new();
        let credentials = directory.0.join("tokens.json");
        fs::create_dir(&credentials).unwrap();
        let error = remove_tokens(&credentials).unwrap_err();
        let io_error = error.downcast_ref::<std::io::Error>().unwrap();
        assert_ne!(io_error.kind(), ErrorKind::NotFound);
        assert!(credentials.is_dir());
    }
}
