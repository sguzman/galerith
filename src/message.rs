//! A typed, serializable foundation for the extractor event protocol.
//!
//! The existing downloader consumes flattened MediaItem values. This module
//! preserves the distinction between directory scope, a downloadable URL,
//! and a nested extractor queue without claiming full gallery-dl parity.
//! The preview adapter below intentionally emits only metadata already known.
use crate::MediaItem;
use serde::Serialize;
use serde_json::{Map, Value};
use std::{collections::HashSet, error::Error, future::Future};

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

/// Shared hard limits for the read-only job-message traverser. These limits
/// do not make the job pipeline equivalent to gallery-dl's downloader.
pub const MAX_QUEUE_DEPTH: u8 = 4;
pub const MAX_QUEUE_CHILDREN: usize = 16;
pub const MAX_JOB_MESSAGES: usize = 8192;

struct JobFrame {
    events: std::vec::IntoIter<ExtractorMessage>,
    inherited: Metadata,
    directory: Metadata,
    depth: u8,
}

impl JobFrame {
    fn new(events: Vec<ExtractorMessage>, inherited: Metadata, depth: u8) -> Self {
        Self {
            events: events.into_iter(),
            inherited,
            directory: Metadata::new(),
            depth,
        }
    }
}

/// Copy scope values only when a message did not provide its own field.
/// Extractor-produced child fields always take precedence over parent fields.
fn inherit_missing(metadata: &mut Metadata, parent: &Metadata) {
    for (key, value) in parent {
        metadata
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
}

/// Inspect a bounded, recursive extractor-message job with an explicit stack.
///
/// Directory events establish scope for subsequent URL/Queue events in that
/// frame. Queue metadata seeds the child frame. Child-defined fields override
/// parent fields. Queues are still emitted when skipped by a limit/cycle. The
/// resolver is responsible for validating URLs and for network rate policies.
///
/// Only the *first* encounter of a queue URL is followed. Globally, at most
/// MAX_QUEUE_CHILDREN children and MAX_JOB_MESSAGES events are permitted.
pub async fn traverse_queue_job_with<F, Fut>(
    events: Vec<ExtractorMessage>,
    max_depth: u8,
    max_children: usize,
    mut resolve_child: F,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>>>,
{
    let depth_limit = max_depth.min(MAX_QUEUE_DEPTH);
    let child_limit = max_children.min(MAX_QUEUE_CHILDREN);
    let mut frames = vec![JobFrame::new(events, Metadata::new(), 0)];
    let mut seen = HashSet::new();
    let mut output = Vec::new();

    while let Some(frame) = frames.last_mut() {
        let Some(mut message) = frame.events.next() else {
            frames.pop();
            continue;
        };
        if output.len() >= MAX_JOB_MESSAGES {
            return Err("extractor job exceeded its message limit".into());
        }

        let mut next = None;
        match &mut message {
            ExtractorMessage::Directory { metadata } => {
                inherit_missing(metadata, &frame.inherited);
                frame.directory = metadata.clone();
            }
            ExtractorMessage::Url { metadata, .. } => {
                inherit_missing(metadata, &frame.directory);
                inherit_missing(metadata, &frame.inherited);
            }
            ExtractorMessage::Queue { url, metadata } => {
                inherit_missing(metadata, &frame.directory);
                inherit_missing(metadata, &frame.inherited);
                if frame.depth < depth_limit
                    && seen.len() < child_limit
                    && seen.insert(url.clone())
                {
                    next = Some((url.clone(), metadata.clone(), frame.depth + 1));
                }
            }
        }
        output.push(message);

        if let Some((url, inherited, child_depth)) = next {
            let child_events = resolve_child(url).await?;
            if child_events.len() > MAX_JOB_MESSAGES.saturating_sub(output.len()) {
                return Err("extractor child exceeded the remaining message limit".into());
            }
            frames.push(JobFrame::new(child_events, inherited, child_depth));
        }
    }
    Ok(output)
}

/// Compatibility wrapper for the original single-level queue inspector.
/// Leaves queue events in the stream and never follows grandchildren.
pub async fn expand_queues_with<F, Fut>(
    events: Vec<ExtractorMessage>,
    max_children: usize,
    resolve_child: F,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>>>,
{
    traverse_queue_job_with(events, 1, max_children, resolve_child).await
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
    #[tokio::test]
    async fn queue_expansion_preserves_parents_and_stops_at_one_level() {
        use std::cell::Cell;

        let calls = Cell::new(0usize);
        let queue = |url: &str| queued_url(url.into(), Metadata::new());
        let events = vec![
            queue("https://example.org/album/a"),
            queue("https://example.org/album/a"),
            queue("https://example.org/album/b"),
        ];
        let expanded = expand_queues_with(events, 1, |url| {
            calls.set(calls.get() + 1);
            async move {
                Ok::<_, Box<dyn Error + Send + Sync>>(vec![queued_url(
                    format!("{url}/grandchild"),
                    Metadata::new(),
                )])
            }
        })
        .await
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(expanded.len(), 4);
        assert!(matches!(expanded[0], ExtractorMessage::Queue { .. }));
        assert_eq!(
            serde_json::to_value(&expanded[1]).unwrap()["url"],
            "https://example.org/album/a/grandchild"
        );
        assert_eq!(
            serde_json::to_value(&expanded[2]).unwrap()["url"],
            "https://example.org/album/a"
        );
        assert_eq!(
            serde_json::to_value(&expanded[3]).unwrap()["url"],
            "https://example.org/album/b"
        );
    }

    #[tokio::test]
    async fn queue_expansion_zero_budget_leaves_all_events_untouched() {
        let events = vec![queued_url(
            "https://example.org/album/a".into(),
            Metadata::new(),
        )];
        let expanded = expand_queues_with(events.clone(), 0, |_| async {
            panic!("zero child budget must not call resolver");
            #[allow(unreachable_code)]
            Ok::<_, Box<dyn Error + Send + Sync>>(Vec::new())
        })
        .await
        .unwrap();
        assert_eq!(expanded, events);
    }

    #[tokio::test]
    async fn queue_expansion_hard_cap_is_sixteen_even_for_large_caller_limit() {
        use std::cell::Cell;

        let calls = Cell::new(0usize);
        let queued: Vec<_> = (0..20)
            .map(|index| {
                queued_url(
                    format!("https://example.org/gallery/{index}"),
                    Metadata::new(),
                )
            })
            .collect();
        let expanded = expand_queues_with(queued.clone(), usize::MAX, |_| {
            calls.set(calls.get() + 1);
            async { Ok::<_, Box<dyn Error + Send + Sync>>(Vec::new()) }
        })
        .await
        .unwrap();
        assert_eq!(calls.get(), 16);
        assert_eq!(expanded, queued);
    }
}
