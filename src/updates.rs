use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
}

pub async fn check() -> String {
    let Ok(repo) = std::env::var("LYRICFLOW_GITHUB_REPO") else {
        return "Set LYRICFLOW_GITHUB_REPO=owner/repo to enable release checks".to_owned();
    };
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        })
    {
        return "Invalid LYRICFLOW_GITHUB_REPO (expected owner/repo)".to_owned();
    }
    let client = match reqwest::Client::builder()
        .user_agent(concat!("lyricflow/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(8))
        .build()
    {
        Ok(client) => client,
        Err(error) => return format!("Update check failed: {error}"),
    };
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let release = match client.get(url).send().await {
        Ok(response) => match response.error_for_status() {
            Ok(response) => match response.json::<Release>().await {
                Ok(release) => release,
                Err(error) => return format!("Update check failed: {error}"),
            },
            Err(error) => return format!("Update check failed: {error}"),
        },
        Err(error) => return format!("Update check failed: {error}"),
    };
    let current = env!("CARGO_PKG_VERSION");
    if release.tag_name.trim_start_matches('v') == current {
        format!("Up to date: v{current}")
    } else {
        format!("Latest: {} | {}", release.tag_name, release.html_url)
    }
}
