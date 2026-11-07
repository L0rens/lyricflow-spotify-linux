use serde::Deserialize;
use std::time::Duration;

const DEFAULT_GITHUB_REPO: &str = "L0rens/lyricflow-spotify-linux";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateResult {
    NotReleased,
    Current,
    Available { version: String, url: String },
    Failed,
    InvalidRepository,
}

impl UpdateResult {
    pub fn message(&self, language: &str) -> String {
        let t = |key| crate::i18n::tr(language, key);
        match self {
            Self::NotReleased => t("update_not_released").to_owned(),
            Self::Current => format!("{}: v{}", t("update_current"), env!("CARGO_PKG_VERSION")),
            Self::Available { version, url } => {
                format!("{}: {} | {}", t("update_available"), version, url)
            }
            Self::Failed => t("update_failed").to_owned(),
            Self::InvalidRepository => t("update_invalid_repo").to_owned(),
        }
    }
}

fn repository(override_repo: Option<String>) -> Result<String, UpdateResult> {
    let repo = override_repo.unwrap_or_else(|| DEFAULT_GITHUB_REPO.to_owned());
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
    {
        return Err(UpdateResult::InvalidRepository);
    }
    Ok(repo)
}

fn evaluate_release(current: &str, release: Release) -> UpdateResult {
    let parse = |value: &str| semver::Version::parse(value.trim_start_matches('v'));
    match (parse(current), parse(&release.tag_name)) {
        (Ok(current), Ok(latest)) if latest > current => UpdateResult::Available {
            version: release.tag_name,
            url: release.html_url,
        },
        (Ok(_), Ok(_)) => UpdateResult::Current,
        _ => UpdateResult::Failed,
    }
}

pub async fn check() -> UpdateResult {
    let repo = match repository(std::env::var("LYRICFLOW_GITHUB_REPO").ok()) {
        Ok(repo) => repo,
        Err(error) => return error,
    };
    let Ok(client) = reqwest::Client::builder()
        .user_agent(concat!("lyricflow/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(8))
        .build()
    else {
        return UpdateResult::Failed;
    };
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let Ok(response) = client.get(url).send().await else {
        return UpdateResult::Failed;
    };
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return UpdateResult::NotReleased;
    }
    let Ok(response) = response.error_for_status() else {
        return UpdateResult::Failed;
    };
    match response.json::<Release>().await {
        Ok(release) => evaluate_release(env!("CARGO_PKG_VERSION"), release),
        Err(_) => UpdateResult::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_repository_and_fork_override_are_validated() {
        assert_eq!(repository(None).unwrap(), DEFAULT_GITHUB_REPO);
        assert_eq!(
            repository(Some("fork/project".into())).unwrap(),
            "fork/project"
        );
        for bad in ["", "a/b/c", "/b", "a/", "https://github.com/a/b", "a/b?x"] {
            assert_eq!(
                repository(Some(bad.into())),
                Err(UpdateResult::InvalidRepository)
            );
        }
    }
    #[test]
    fn version_comparison_does_not_call_older_releases_updates() {
        let release = |tag: &str| Release {
            tag_name: tag.into(),
            html_url: "https://github.com/example/release".into(),
        };
        assert!(matches!(
            evaluate_release("0.1.0", release("v0.2.0")),
            UpdateResult::Available { .. }
        ));
        assert_eq!(
            evaluate_release("0.2.0", release("v0.1.0")),
            UpdateResult::Current
        );
        assert_eq!(
            evaluate_release("0.2.0", release("v0.2.0")),
            UpdateResult::Current
        );
        assert_eq!(
            evaluate_release("0.2.0", release("v0.2.0-rc.1")),
            UpdateResult::Current
        );
        assert_eq!(
            evaluate_release("0.2.0", release("invalid")),
            UpdateResult::Failed
        );
        assert!(!UpdateResult::NotReleased.message("en").is_empty());
    }
}
