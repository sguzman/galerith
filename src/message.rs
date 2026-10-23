//! A typed, serializable foundation for the extractor event protocol.
//!
//! The existing downloader consumes flattened MediaItem values. This module
//! preserves the distinction between directory scope, a downloadable URL,
//! and a nested extractor queue without claiming full gallery-dl parity.
//! The preview adapter below intentionally emits only metadata already known.
use crate::MediaItem;
use serde::Serialize;
use serde_json::{Map, Value};

pub type Metadata = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtractorMessage {
    Directory { metadata: Metadata },
    Url { url: String, metadata: Metadata },
    Queue { url: String, metadata: Metadata },
}

pub fn media_metadata(item: &MediaItem) -> Metadata {
    let mut metadata = Metadata::new();
    metadata.insert("category".into(), item.extractor.into());
    metadata.insert("extractor".into(), item.extractor.into());
    metadata.insert("filename".into(), item.filename.clone().into());
    if let Some(id) = item.post_id {
        metadata.insert("post_id".into(), id.into());
    }
    metadata
}

/// Wrap a *flattened preview* in directory/URL events. This does not recover
/// lost upstream post, page, revision or gallery metadata.
pub fn from_media(items: Vec<MediaItem>) -> Vec<ExtractorMessage> {
    let mut result = Vec::with_capacity(items.len().saturating_mul(2));
    for item in items {
        let metadata = media_metadata(&item);
        result.push(ExtractorMessage::Directory {
            metadata: metadata.clone(),
        });
        result.push(ExtractorMessage::Url {
            url: item.url,
            metadata,
        });
    }
    result
}

/// Create a deferred child extraction event without consuming child media.
/// The caller is responsible for URL validation and for enforcing queue limits.
pub fn queued_url(url: String, metadata: Metadata) -> ExtractorMessage {
    ExtractorMessage::Queue { url, metadata }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> MediaItem {
        MediaItem {
            url: "https://example.org/full/test.jpg".into(),
            filename: "test.jpg".into(),
            extractor: "directlink",
            post_id: Some(42),
        }
    }

    #[test]
    fn one_media_item_has_ordered_directory_and_url_events() {
        let events = from_media(vec![fixture()]);
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_value(&events[0]).unwrap(),
            json!({"type":"directory","metadata":{
                "category":"directlink","extractor":"directlink",
                "filename":"test.jpg","post_id":42
            }})
        );
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap(),
            json!({"type":"url","url":"https://example.org/full/test.jpg","metadata":{
                "category":"directlink","extractor":"directlink",
                "filename":"test.jpg","post_id":42
            }})
        );
    }

    #[test]
    fn queue_events_are_distinct_from_media_and_do_not_emit_urls() {
        let mut metadata = Metadata::new();
        metadata.insert("collection_id".into(), json!("12345"));
        let event = queued_url(
            "https://wallhaven.cc/user/example/favorites/12345".into(),
            metadata,
        );
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({
                "type":"queue",
                "url":"https://wallhaven.cc/user/example/favorites/12345",
                "metadata":{"collection_id":"12345"}
            })
        );
    }

    #[test]
    fn empty_input_produces_no_events_and_none_post_id_is_omitted() {
        assert!(from_media(Vec::new()).is_empty());
        let mut item = fixture();
        item.post_id = None;
        let events = from_media(vec![item]);
        let value = serde_json::to_value(&events[1]).unwrap();
        assert!(value["metadata"].get("post_id").is_none());
    }
}
