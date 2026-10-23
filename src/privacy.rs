//! Do not print signed URLs, session tokens, or query strings in errors.
//! Detailed parser and network errors can include credentials from request URLs.
use std::error::Error;
use url::Url;

pub fn display_url(input: &str) -> String {
    let Ok(mut url) = Url::parse(input) else {
        return "[invalid URL]".into();
    };
    if !matches!(url.scheme(), "http" | "https") {
        return "[unsupported URL scheme]".into();
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

pub fn display_error(error: &(dyn Error + 'static)) -> &'static str {
    if let Some(error) = error.downcast_ref::<reqwest::Error>() {
        if error.is_timeout() {
            return "network timeout";
        }
        if error.is_connect() {
            return "connection failed";
        }
        if error.status().is_some() {
            return "HTTP status error";
        }
        return "HTTP request or response failed";
    }
    if let Some(error) = error.downcast_ref::<crate::ExtractError>() {
        return match error {
            crate::ExtractError::InvalidUrl => "invalid URL",
            crate::ExtractError::UnsupportedUrl => "no extractor matches this URL",
        };
    }
    if error.is::<std::io::Error>() {
        return "filesystem operation failed";
    }
    "extraction or download failed"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_auth_query_and_fragment() {
        assert_eq!(
            display_url("https://user:password@example.test:8080/image.jpg?token=secret#key"),
            "https://example.test:8080/image.jpg"
        );
        assert_eq!(display_url("not-a-url"), "[invalid URL]");
        assert_eq!(
            display_url("file:///private/secret.png"),
            "[unsupported URL scheme]"
        );
    }

    #[test]
    fn error_classification_does_not_include_private_causes() {
        let error = std::io::Error::other("signed_token=secret");
        assert_eq!(display_error(&error), "filesystem operation failed");
    }
}
