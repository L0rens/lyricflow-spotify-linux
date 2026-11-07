use reqwest::{Response, StatusCode};
use std::{fmt, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpotifyError {
    Configuration,
    NotConnected,
    Authorization,
    Forbidden,
    RateLimited(Duration),
    Unavailable,
    Timeout,
    InvalidResponse,
    Storage,
}

impl SpotifyError {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Configuration => "spotify_configuration",
            Self::NotConnected => "spotify_not_connected",
            Self::Authorization => "spotify_authorization_expired",
            Self::Forbidden => "spotify_forbidden",
            Self::RateLimited(_) => "spotify_rate_limited",
            Self::Unavailable => "spotify_unavailable",
            Self::Timeout => "spotify_timeout",
            Self::InvalidResponse => "spotify_invalid_response",
            Self::Storage => "spotify_storage_error",
        }
    }

    pub fn needs_login(&self) -> bool {
        matches!(
            self,
            Self::Configuration | Self::NotConnected | Self::Authorization | Self::Storage
        )
    }

    pub fn from_status(status: StatusCode, retry_after: Option<&str>, token_request: bool) -> Self {
        match status.as_u16() {
            400 if token_request => Self::Authorization,
            401 => Self::Authorization,
            403 => Self::Forbidden,
            429 => Self::RateLimited(Duration::from_secs(
                retry_after
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(60)
                    .max(1),
            )),
            500..=599 => Self::Unavailable,
            _ => Self::InvalidResponse,
        }
    }
}

impl fmt::Display for SpotifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", crate::i18n::tr("en", self.key()))
    }
}

impl std::error::Error for SpotifyError {}

pub async fn checked(
    response: Result<Response, reqwest::Error>,
    token_request: bool,
) -> Result<Response, SpotifyError> {
    let response = response.map_err(|error| {
        if error.is_timeout() {
            SpotifyError::Timeout
        } else {
            SpotifyError::Unavailable
        }
    })?;
    if response.status().is_success() {
        return Ok(response);
    }
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok());
    Err(SpotifyError::from_status(
        response.status(),
        retry_after,
        token_request,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn http_response_headers_drive_backoff_without_exposing_the_body() {
        let response = http::Response::builder()
            .status(429)
            .header("Retry-After", "97")
            .body("credential-like-response-body-that-must-stay-private")
            .unwrap();
        let error = checked(Ok(response.into()), false).await.unwrap_err();
        assert_eq!(error, SpotifyError::RateLimited(Duration::from_secs(97)));
        assert!(!error.to_string().contains("credential-like"));
        let response = http::Response::builder()
            .status(400)
            .body("sensitive invalid_grant body")
            .unwrap();
        assert_eq!(
            checked(Ok(response.into()), true).await.unwrap_err(),
            SpotifyError::Authorization
        );
        let response = http::Response::builder().status(204).body("").unwrap();
        assert_eq!(
            checked(Ok(response.into()), false).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
    }

    #[test]
    fn http_errors_are_actionable_and_preserve_rate_limit_delay() {
        assert_eq!(
            SpotifyError::from_status(StatusCode::UNAUTHORIZED, None, false),
            SpotifyError::Authorization
        );
        assert_eq!(
            SpotifyError::from_status(StatusCode::FORBIDDEN, None, false),
            SpotifyError::Forbidden
        );
        assert_eq!(
            SpotifyError::from_status(StatusCode::BAD_REQUEST, None, true),
            SpotifyError::Authorization
        );
        assert_eq!(
            SpotifyError::from_status(StatusCode::SERVICE_UNAVAILABLE, None, false),
            SpotifyError::Unavailable
        );
        assert_eq!(
            SpotifyError::from_status(StatusCode::TOO_MANY_REQUESTS, Some("123"), false),
            SpotifyError::RateLimited(Duration::from_secs(123))
        );
        for value in [None, Some("invalid")] {
            assert_eq!(
                SpotifyError::from_status(StatusCode::TOO_MANY_REQUESTS, value, false),
                SpotifyError::RateLimited(Duration::from_secs(60))
            );
        }
        assert!(SpotifyError::Authorization.needs_login());
        assert!(!SpotifyError::Forbidden.needs_login());
    }
}
