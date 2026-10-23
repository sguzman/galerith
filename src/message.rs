//! A typed, serializable foundation for the extractor event protocol.
//!
//! The existing downloader consumes flattened MediaItem values. This module
//! preserves the distinction between directory scope, a downloadable URL,
//! and a nested extractor queue without claiming full gallery-dl parity.
//! The preview adapter below intentionally emits only metadata already known.
use crate::{range::IndexRange, MediaItem};
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

/// Zero-based internal filtering is forbidden: both sequences use the
/// existing positive, one-based inclusive/slice selector syntax.
/// Ordinals are global to a single root extraction, not reset by a child.
#[derive(Debug, Clone, Default)]
pub struct JobSelection {
    pub files: Option<IndexRange>,
    pub children: Option<IndexRange>,
}

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
        metadata.entry(key.clone()).or_insert_with(|| value.clone());
    }
}

/// Per-file identity fields must not leak into unrelated child extractors.
/// Contextual fields such as collection_id and username may be inherited.
fn child_context(metadata: &Metadata) -> Metadata {
    let mut context = metadata.clone();
    for key in ["filename", "extractor", "post_id"] {
        context.remove(key);
    }
    context
}

/// Treat equivalent absolute URLs as one queue cycle, while preserving the
/// original URL in every emitted event. Query parameters remain significant.
fn queue_identity(input: &str) -> String {
    match url::Url::parse(input) {
        Ok(mut parsed) => {
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => input.to_owned(),
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
/// Traverse with optional file/child selectors. A child selector is
/// evaluated before HTTP resolution; a skipped queue is still emitted as
/// a control message. A file selector drops unselected URL events before
/// they can become downloader candidates. Neither selector invents posts.
pub async fn traverse_queue_job_filtered_with<F, Fut>(
    events: Vec<ExtractorMessage>,
    max_depth: u8,
    max_children: usize,
    selection: &JobSelection,
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
    let mut scanned = 0usize;
    let mut file_ordinal = 0usize;
    let mut child_ordinal = 0usize;

    while let Some(frame) = frames.last_mut() {
        let Some(mut message) = frame.events.next() else {
            frames.pop();
            continue;
        };
        scanned += 1;
        if scanned > MAX_JOB_MESSAGES {
            return Err("extractor job exceeded its message limit".into());
        }

        let mut next = None;
        let mut emit = true;
        match &mut message {
            ExtractorMessage::Directory { metadata } => {
                inherit_missing(metadata, &frame.inherited);
                frame.directory = metadata.clone();
            }
            ExtractorMessage::Url { metadata, .. } => {
                inherit_missing(metadata, &frame.directory);
                inherit_missing(metadata, &frame.inherited);
                file_ordinal += 1;
                emit = selection
                    .files
                    .as_ref()
                    .is_none_or(|range| range.contains(file_ordinal));
            }
            ExtractorMessage::Queue { url, metadata } => {
                inherit_missing(metadata, &frame.directory);
                inherit_missing(metadata, &frame.inherited);
                child_ordinal += 1;
                if selection
                    .children
                    .as_ref()
                    .is_none_or(|range| range.contains(child_ordinal))
                    && frame.depth < depth_limit
                    && seen.len() < child_limit
                    && seen.insert(queue_identity(url))
                {
                    next = Some((url.clone(), child_context(metadata), frame.depth + 1));
                }
            }
        }
        if emit {
            output.push(message);
        }

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

/// Unfiltered compatibility entry point for the established job API.
pub async fn traverse_queue_job_with<F, Fut>(
    events: Vec<ExtractorMessage>,
    max_depth: u8,
    max_children: usize,
    resolve_child: F,
) -> Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Vec<ExtractorMessage>, Box<dyn Error + Send + Sync>>>,
{
    traverse_queue_job_filtered_with(
        events,
        max_depth,
        max_children,
        &JobSelection::default(),
        resolve_child,
    )
    .await
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

    #[tokio::test]
    async fn iterative_job_tracks_directory_scope_and_child_metadata_precedence() {
        use std::cell::Cell;

        let calls = Cell::new(0);
        let root = vec![
            ExtractorMessage::Directory {
                metadata: serde_json::from_value(json!({
                    "category":"parent","origin":"root"
                }))
                .unwrap(),
            },
            queued_url(
                "https://example.org/a".into(),
                serde_json::from_value(json!({
                    "collection_id":"a","category":"wallhaven"
                }))
                .unwrap(),
            ),
            ExtractorMessage::Url {
                url: "https://example.org/root.jpg".into(),
                metadata: Metadata::new(),
            },
        ];
        let events = traverse_queue_job_with(root, 2, 3, |url| {
            calls.set(calls.get() + 1);
            async move {
                let items = if url.ends_with("/a") {
                    vec![
                        ExtractorMessage::Directory {
                            metadata: serde_json::from_value(json!({
                                "category":"child","album":"album A"
                            }))
                            .unwrap(),
                        },
                        ExtractorMessage::Url {
                            url: "https://example.org/a.jpg".into(),
                            metadata: Metadata::new(),
                        },
                        queued_url("https://example.org/b".into(), Metadata::new()),
                    ]
                } else {
                    vec![
                        ExtractorMessage::Url {
                            url: "https://example.org/b.jpg".into(),
                            metadata: serde_json::from_value(json!({
                                "category":"grandchild","filename":"b.jpg"
                            }))
                            .unwrap(),
                        },
                        // The cycle remains visible but is not traversed.
                        queued_url("https://example.org/a".into(), Metadata::new()),
                    ]
                };
                Ok::<_, Box<dyn Error + Send + Sync>>(items)
            }
        })
        .await
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(events.len(), 8);
        let as_json: Vec<Value> = events
            .iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect();
        assert_eq!(as_json[1]["type"], "queue");
        assert_eq!(as_json[1]["metadata"]["origin"], "root");
        assert_eq!(as_json[2]["metadata"]["category"], "child");
        assert_eq!(as_json[2]["metadata"]["collection_id"], "a");
        assert_eq!(as_json[3]["metadata"]["album"], "album A");
        assert_eq!(as_json[3]["metadata"]["origin"], "root");
        assert_eq!(as_json[4]["url"], "https://example.org/b");
        assert_eq!(as_json[5]["metadata"]["category"], "grandchild");
        assert_eq!(as_json[5]["metadata"]["collection_id"], "a");
        assert_eq!(as_json[6]["url"], "https://example.org/a");
        assert_eq!(as_json[7]["url"], "https://example.org/root.jpg");
        // Parent scope must not be replaced by a child's active directory.
        assert_eq!(as_json[7]["metadata"]["category"], "parent");
        assert_eq!(as_json[7]["metadata"]["origin"], "root");
        assert!(as_json[7]["metadata"].get("album").is_none());
    }

    #[tokio::test]
    async fn queue_depth_zero_and_one_never_fetch_grandchildren() {
        use std::cell::Cell;

        for depth in [0, 1, 2] {
            let calls = Cell::new(0);
            let root = vec![queued_url("https://example.org/a".into(), Metadata::new())];
            let events = traverse_queue_job_with(root, depth, 16, |url| {
                calls.set(calls.get() + 1);
                async move {
                    Ok::<_, Box<dyn Error + Send + Sync>>(vec![queued_url(
                        format!("{url}/child"),
                        Metadata::new(),
                    )])
                }
            })
            .await
            .unwrap();
            assert_eq!(calls.get(), usize::from(depth));
            assert_eq!(events.len(), usize::from(depth) + 1);
        }
    }

    #[tokio::test]
    async fn event_limit_rejects_oversized_root_before_adding_more() {
        let events = (0..=MAX_JOB_MESSAGES)
            .map(|_| ExtractorMessage::Directory {
                metadata: Metadata::new(),
            })
            .collect();
        let err = traverse_queue_job_with(events, 0, 0, |_| async {
            Ok::<_, Box<dyn Error + Send + Sync>>(Vec::new())
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("message limit"));
    }

    #[tokio::test]
    async fn excessive_child_batch_is_rejected_before_it_is_inserted() {
        let root = vec![queued_url("https://example.org/a".into(), Metadata::new())];
        let err = traverse_queue_job_with(root, 1, 1, |_| async {
            Ok::<_, Box<dyn Error + Send + Sync>>(
                (0..MAX_JOB_MESSAGES)
                    .map(|_| ExtractorMessage::Directory {
                        metadata: Metadata::new(),
                    })
                    .collect(),
            )
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("remaining message limit"));
    }

    #[tokio::test]
    async fn child_context_never_inherits_parent_media_identity() {
        let root = vec![
            ExtractorMessage::Directory {
                metadata: serde_json::from_value(json!({
                    "category":"parent",
                    "filename":"parent.jpg",
                    "extractor":"directlink",
                    "post_id":123,
                    "collection_id":"keep-me"
                }))
                .unwrap(),
            },
            queued_url("https://example.org/child".into(), Metadata::new()),
        ];
        let events = traverse_queue_job_with(root, 1, 1, |_| async {
            Ok::<_, Box<dyn Error + Send + Sync>>(vec![ExtractorMessage::Url {
                url: "https://example.org/child.jpg".into(),
                metadata: Metadata::new(),
            }])
        })
        .await
        .unwrap();
        let queued = serde_json::to_value(&events[1]).unwrap();
        assert_eq!(queued["metadata"]["filename"], "parent.jpg");
        let child = serde_json::to_value(&events[2]).unwrap();
        assert_eq!(child["metadata"]["category"], "parent");
        assert_eq!(child["metadata"]["collection_id"], "keep-me");
        assert!(child["metadata"].get("filename").is_none());
        assert!(child["metadata"].get("extractor").is_none());
        assert!(child["metadata"].get("post_id").is_none());
    }

    #[tokio::test]
    async fn equivalent_queue_urls_share_one_cycle_identity() {
        use std::cell::Cell;
        let calls = Cell::new(0usize);
        let inputs = vec![
            queued_url(
                "https://EXAMPLE.org/album?id=1#first".into(),
                Metadata::new(),
            ),
            queued_url("https://example.org/album?id=1".into(), Metadata::new()),
            queued_url("https://example.org/album?id=2".into(), Metadata::new()),
        ];
        let output = traverse_queue_job_with(inputs, 1, 16, |_| {
            calls.set(calls.get() + 1);
            async { Ok::<_, Box<dyn Error + Send + Sync>>(Vec::new()) }
        })
        .await
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(output.len(), 3);
        assert_eq!(
            serde_json::to_value(&output[0]).unwrap()["url"],
            "https://EXAMPLE.org/album?id=1#first"
        );
    }

    #[tokio::test]
    async fn job_selectors_skip_child_http_and_filter_files_before_download() {
        use std::cell::Cell;

        let calls = Cell::new(0usize);
        let roots = vec![
            ExtractorMessage::Url {
                url: "https://example.org/first.jpg".into(),
                metadata: Metadata::new(),
            },
            queued_url("https://example.org/skip".into(), Metadata::new()),
            queued_url("https://example.org/keep".into(), Metadata::new()),
            ExtractorMessage::Url {
                url: "https://example.org/last.jpg".into(),
                metadata: Metadata::new(),
            },
        ];
        let selected = JobSelection {
            files: Some(IndexRange::parse("2").unwrap()),
            children: Some(IndexRange::parse("2").unwrap()),
        };
        let output = traverse_queue_job_filtered_with(roots, 2, 8, &selected, |url| {
            calls.set(calls.get() + 1);
            async move {
                assert_eq!(url, "https://example.org/keep");
                Ok::<_, Box<dyn Error + Send + Sync>>(vec![ExtractorMessage::Url {
                    url: "https://example.org/from-child.jpg".into(),
                    metadata: Metadata::new(),
                }])
            }
        })
        .await
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(output.len(), 3);
        let json: Vec<Value> = output
            .iter()
            .map(|value| serde_json::to_value(value).unwrap())
            .collect();
        assert_eq!(json[0]["url"], "https://example.org/skip");
        assert_eq!(json[1]["url"], "https://example.org/keep");
        assert_eq!(json[2]["url"], "https://example.org/from-child.jpg");
    }

    #[tokio::test]
    async fn child_range_counts_duplicates_without_spending_selected_request_budget() {
        use std::cell::Cell;

        let calls = Cell::new(0usize);
        let roots = vec![
            queued_url("https://example.org/a".into(), Metadata::new()),
            queued_url("https://example.org/a#different".into(), Metadata::new()),
            queued_url("https://example.org/b".into(), Metadata::new()),
        ];
        let selected = JobSelection {
            files: None,
            children: Some(IndexRange::parse("2-3").unwrap()),
        };
        let output = traverse_queue_job_filtered_with(roots, 1, 2, &selected, |url| {
            calls.set(calls.get() + 1);
            async move {
                Ok::<_, Box<dyn Error + Send + Sync>>(vec![ExtractorMessage::Url {
                    url: format!("{url}/file.jpg"),
                    metadata: Metadata::new(),
                }])
            }
        })
        .await
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(output.len(), 5);
        // The first skipped URL does not reserve a slot in the visited set.
        assert!(matches!(output[0], ExtractorMessage::Queue { .. }));
        assert!(matches!(output[1], ExtractorMessage::Queue { .. }));
    }

    #[tokio::test]
    async fn unselected_media_events_cannot_exceed_global_processed_event_limit() {
        let events = (0..=MAX_JOB_MESSAGES)
            .map(|_| ExtractorMessage::Url {
                url: "https://example.org/item.jpg".into(),
                metadata: Metadata::new(),
            })
            .collect();
        let selection = JobSelection {
            files: Some(IndexRange::parse("1").unwrap()),
            children: None,
        };
        let error = traverse_queue_job_filtered_with(events, 0, 0, &selection, |_| async {
            Ok::<_, Box<dyn Error + Send + Sync>>(Vec::new())
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("message limit"));
    }

    #[tokio::test]
    async fn filtering_one_file_keeps_its_directory_context_event() {
        let media = vec![
            MediaItem {
                url: "https://example.org/one.jpg".into(),
                filename: "one.jpg".into(),
                extractor: "directlink",
                post_id: Some(1),
            },
            MediaItem {
                url: "https://example.org/two.jpg".into(),
                filename: "two.jpg".into(),
                extractor: "directlink",
                post_id: Some(2),
            },
        ];
        let selection = JobSelection {
            files: Some(IndexRange::parse("2").unwrap()),
            children: None,
        };
        let output =
            traverse_queue_job_filtered_with(from_media(media), 0, 0, &selection, |_| async {
                Ok::<_, Box<dyn Error + Send + Sync>>(Vec::new())
            })
            .await
            .unwrap();
        assert_eq!(output.len(), 3);
        assert!(matches!(output[0], ExtractorMessage::Directory { .. }));
        assert!(matches!(output[1], ExtractorMessage::Directory { .. }));
        assert!(matches!(output[2], ExtractorMessage::Url { .. }));
        assert_eq!(
            serde_json::to_value(&output[2]).unwrap()["url"],
            "https://example.org/two.jpg"
        );
    }
}
