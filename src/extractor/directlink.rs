//! Typed metadata preview for direct HTTP(S) media links.
//! The downloader still uses a safe flat filename. Rich fields here do not
//! imply full upstream formatting, header or archive interoperability.

use crate::{
    extract_direct_media,
    message::{media_metadata, ExtractorMessage},
    ExtractError,
};
use percent_encoding::percent_decode_str;
use url::Url;

pub fn resolve_messages(input: &str) -> Result<Vec<ExtractorMessage>, ExtractError> {
    let media = extract_direct_media(input)?;
    let parsed = Url::parse(&media.url).map_err(|_| ExtractError::InvalidUrl)?;
    let domain = parsed.host_str().ok_or(ExtractError::UnsupportedUrl)?;
    let subdomain: Vec<_> = domain.rsplit('.').take(2).collect();
    let subcategory = subdomain.into_iter().rev().collect::<Vec<_>>().join(".");
    let directory = parsed
        .path()
        .trim_start_matches('/')
        .rsplit_once('/')
        .map_or("", |(path, _)| path);
    let path = percent_decode_str(directory)
        .decode_utf8_lossy()
        .into_owned();
    let (stem, extension) = media
        .filename
        .rsplit_once('.')
        .ok_or(ExtractError::UnsupportedUrl)?;

    let mut metadata = media_metadata(&media);
    metadata.insert("domain".into(), domain.into());
    metadata.insert("subcategory".into(), subcategory.into());
    metadata.insert("path".into(), path.into());
    metadata.insert("stem".into(), stem.into());
    metadata.insert("extension".into(), extension.to_ascii_lowercase().into());
    if let Some(query) = parsed.query() {
        metadata.insert(
            "query".into(),
            percent_decode_str(query)
                .decode_utf8_lossy()
                .into_owned()
                .into(),
        );
    }
    if let Some(fragment) = parsed.fragment() {
        metadata.insert(
            "fragment".into(),
            percent_decode_str(fragment)
                .decode_utf8_lossy()
                .into_owned()
                .into(),
        );
    }
    Ok(vec![
        ExtractorMessage::Directory {
            metadata: metadata.clone(),
        },
        ExtractorMessage::Url {
            url: media.url,
            metadata,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_and_url_keep_direct_link_identity_and_optional_fields() {
        let events =
            resolve_messages("https://assets.example.org/arts/Poster%20Art.JPEG?download=1#first")
                .unwrap();
        assert_eq!(events.len(), 2);
        let directory = serde_json::to_value(&events[0]).unwrap();
        let url = serde_json::to_value(&events[1]).unwrap();
        assert_eq!(directory["type"], "directory");
        assert_eq!(url["type"], "url");
        assert_eq!(directory["metadata"], url["metadata"]);
        assert_eq!(url["metadata"]["category"], "directlink");
        assert_eq!(url["metadata"]["domain"], "assets.example.org");
        assert_eq!(url["metadata"]["subcategory"], "example.org");
        assert_eq!(url["metadata"]["path"], "arts");
        assert_eq!(url["metadata"]["filename"], "Poster Art.JPEG");
        assert_eq!(url["metadata"]["stem"], "Poster Art");
        assert_eq!(url["metadata"]["extension"], "jpeg");
        assert_eq!(url["metadata"]["query"], "download=1");
        assert_eq!(url["metadata"]["fragment"], "first");
    }

    #[test]
    fn root_directory_and_path_separators_remain_data_only() {
        let events = resolve_messages("https://example.org/%2E%2E%2Fsecret.jpg").unwrap();
        let value = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(value["metadata"]["path"], "");
        assert_eq!(value["metadata"]["filename"], ".._secret.jpg");
        assert_eq!(value["metadata"]["subcategory"], "example.org");
        assert!(value["metadata"].get("query").is_none());
    }
}
