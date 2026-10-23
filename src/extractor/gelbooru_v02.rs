//! Gelbooru Beta 0.2 XML/DAPI read-only preview.
//! The pinned upstream module also implements pools, favorites, rate policies,
//! tag metadata, alternate media URLs and site-specific auth. Those remain open.
use crate::{safe_filename, MediaItem};
use reqwest::Client;
use std::{collections::HashMap, error::Error, time::Duration};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Post { host: String, id: u64 },
    Search { host: String, tags: String },
}

fn category(host: &str) -> Option<&'static str> {
    match host {
        "rule34.xxx" => Some("rule34"),
        "safebooru.org" => Some("safebooru"),
        "tbib.org" => Some("tbib"),
        "hypnohub.net" => Some("hypnohub"),
        "xbooru.com" => Some("xbooru"),
        _ => None,
    }
}

pub fn match_url(input: &str) -> Option<Target> {
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https" || url.path() != "/index.php" {
        return None;
    }
    let host = url.host_str()?;
    category(host)?;
    let params: HashMap<_, _> = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    if params.get("page")?.as_str() != "post" {
        return None;
    }
    match params.get("s")?.as_str() {
        "view" => {
            let id = params.get("id")?.parse::<u64>().ok()?;
            (id > 0).then_some(Target::Post {
                host: host.into(),
                id,
            })
        }
        "list" => Some(Target::Search {
            host: host.into(),
            tags: params.get("tags")?.clone(),
        }),
        _ => None,
    }
}

fn read_xml(data: &str, host: &str) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let document = roxmltree::Document::parse(data)?;
    let root = document.root_element();
    if !root.has_tag_name("posts") {
        return Err("unrecognized Gelbooru v0.2 XML response".into());
    }
    let mut items = Vec::new();
    for post in root.children().filter(|node| node.has_tag_name("post")) {
        let Some(id) = post.attribute("id").and_then(|raw| raw.parse::<u64>().ok()) else {
            continue;
        };
        let Some(raw_url) = post.attribute("file_url") else {
            continue;
        };
        let Ok(url) = Url::parse(raw_url) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        let Some(extension) = url.path().rsplit_once('.').map(|(_, ext)| ext) else {
            continue;
        };
        if extension.is_empty()
            || extension.len() > 12
            || !extension.bytes().all(|ch| ch.is_ascii_alphanumeric())
        {
            continue;
        }
        let extractor = category(host).ok_or("unsupported Gelbooru v0.2 host")?;
        items.push(MediaItem {
            url: url.to_string(),
            filename: safe_filename(&format!("{id}.{extension}")),
            extractor,
            post_id: Some(id),
        });
    }
    Ok(items)
}

pub async fn resolve(
    client: &Client,
    target: Target,
    max_pages: u16,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let host = match &target {
        Target::Post { host, .. } | Target::Search { host, .. } => host,
    };
    let api_host = if host == "rule34.xxx" {
        "api.rule34.xxx"
    } else {
        host.as_str()
    };
    let api = Url::parse(&format!("https://{api_host}/"))?;
    resolve_at_base(client, target, max_pages, &api).await
}

async fn resolve_at_base(
    client: &Client,
    target: Target,
    max_pages: u16,
    api: &Url,
) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
    let host = match &target {
        Target::Post { host, .. } | Target::Search { host, .. } => host.as_str(),
    };
    let pages = if matches!(target, Target::Post { .. }) {
        1
    } else {
        max_pages.max(1)
    };
    let mut items = Vec::new();
    for page in 0..pages {
        let mut endpoint = api.join("index.php")?;
        {
            let mut query = endpoint.query_pairs_mut();
            query
                .append_pair("page", "dapi")
                .append_pair("s", "post")
                .append_pair("q", "index");
            match &target {
                Target::Post { id, .. } => {
                    query.append_pair("id", &id.to_string());
                }
                Target::Search { tags, .. } => {
                    query
                        .append_pair("tags", tags)
                        .append_pair("limit", "100")
                        .append_pair("pid", &page.to_string());
                }
            }
        }
        let response = client.get(endpoint).send().await?.error_for_status()?;
        let body = response.text().await?;
        let fresh = read_xml(&body, host)?;
        let done = matches!(target, Target::Post { .. }) || fresh.len() < 100;
        items.extend(fresh);
        if done {
            break;
        }
        if page + 1 < pages {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_five_pinned_xml_api_hosts_only() {
        for host in [
            "rule34.xxx",
            "safebooru.org",
            "tbib.org",
            "hypnohub.net",
            "xbooru.com",
        ] {
            assert_eq!(
                match_url(&format!("https://{host}/index.php?id=11&s=view&page=post")),
                Some(Target::Post {
                    host: host.into(),
                    id: 11
                }),
            );
        }
        assert_eq!(
            match_url("https://safebooru.org/index.php?tags=blue+sky&s=list&page=post"),
            Some(Target::Search {
                host: "safebooru.org".into(),
                tags: "blue sky".into()
            }),
        );
        assert_eq!(
            match_url("https://safebooru.donmai.us/index.php?page=post&s=view&id=1"),
            None
        );
        assert_eq!(
            match_url("http://tbib.org/index.php?page=post&s=view&id=1"),
            None
        );
        assert_eq!(
            match_url("https://rule34.xxx/index.php?page=pool&s=show&id=1"),
            None
        );
    }

    #[test]
    fn parses_xml_and_ignores_unavailable_or_unsafe_media() {
        let xml = r#"<posts count="3">
          <post id="12" file_url="https://cdn.example.test/a.jpg?token=one&amp;b=two"/>
          <post id="13" file_url="javascript:unsafe()" />
          <post id="14" file_url="" />
          <post id="15" file_url="https://cdn.example.test/video.webm" />
        </posts>"#;
        let files = read_xml(xml, "safebooru.org").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].filename, "12.jpg");
        assert_eq!(files[1].filename, "15.webm");
        assert_eq!(files[0].extractor, "safebooru");
        assert!(read_xml("<error>denied</error>", "safebooru.org").is_err());
        assert!(read_xml("not xml", "safebooru.org").is_err());
    }

    #[tokio::test]
    async fn post_query_uses_xml_dapi_endpoint_on_synthetic_server() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let server_socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = server_socket.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = server_socket.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let read = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..read]);
            assert!(request.starts_with("GET /index.php?"), "{request}");
            assert!(request.contains("page=dapi"), "{request}");
            assert!(request.contains("q=index"), "{request}");
            assert!(request.contains("id=44"), "{request}");
            let xml = r#"<posts count="1"><post id="44" file_url="https://cdn.example.test/photo.png"/></posts>"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
                xml.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let api = Url::parse(&format!("http://{address}/")).unwrap();
        let target = Target::Post {
            host: "tbib.org".into(),
            id: 44,
        };
        let output = resolve_at_base(&Client::new(), target, 1, &api)
            .await
            .unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].filename, "44.png");
        server.await.unwrap();
    }
    #[tokio::test]
    async fn tag_search_uses_xml_dapi_and_stops_on_short_page() {
        use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            let length = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..length]);
            assert!(request.starts_with("GET /index.php?"), "{request}");
            assert!(request.contains("tags=blue+sky"), "{request}");
            assert!(request.contains("pid=0"), "{request}");
            assert!(request.contains("limit=100"), "{request}");
            let xml = r#"<posts count="1"><post id="18" file_url="https://img.example.test/a.jpg"/></posts>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}", xml.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let api = Url::parse(&format!("http://{addr}/")).unwrap();
        let target = Target::Search {
            host: "xbooru.com".into(),
            tags: "blue sky".into(),
        };
        let files = resolve_at_base(&Client::new(), target, 3, &api).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "18.jpg");
        assert_eq!(files[0].extractor, "xbooru");
        server.await.unwrap();
    }
}
