//! Guarded public Architizer project media and firm-to-project queues.
use crate::{message::{media_metadata, queued_url, ExtractorMessage, Metadata}, safe_filename, MediaItem};
use reqwest::Client;
use std::{collections::HashSet, error::Error};
use url::Url;

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_MEDIA: usize = 160;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target { Project(String), Firm(String) }

fn slug(s: &str) -> bool {
    (1..=100).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub fn match_url(raw: &str) -> Option<Target> {
    let candidate = if raw.starts_with("architizer.com/") {
        format!("https://{raw}")
    } else { raw.to_owned() };
    let lower = candidate.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"].iter().any(|v| lower.contains(v)) {
        return None;
    }
    let url = Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str() != Some("architizer.com")
        || url.port().is_some() || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let paths: Vec<_> = url.path_segments()?.collect();
    match paths.as_slice() {
        ["projects", project] | ["projects", project, ""] if slug(project) =>
            Some(Target::Project((*project).into())),
        ["firms", firm] | ["firms", firm, ""] if slug(firm) =>
            Some(Target::Firm((*firm).into())),
        _ => None,
    }
}

fn between<'a>(page: &'a str, start: &str, end: &str) -> Option<&'a str> {
    page.split_once(start)?.1.split_once(end).map(|(text, _)| text)
}

fn clean(value: &str) -> Option<String> {
    if value.len() > 2048 || value.chars().any(char::is_control) { return None; }
    let decoded = value.replace("&amp;", "&").replace("&quot;", "\"")
        .replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">");
    (!decoded.contains('<')).then_some(decoded)
}

fn project_meta(html: &str) -> Metadata {
    let body = html.split_once("id=\"Pages\"").map(|(_, tail)| tail).unwrap_or(html);
    let mut result = Metadata::new();
    for (key, start, end) in [
        ("title", "data-name='", "'"),
        ("slug", "data-slug='", "'"),
        ("firm", "data-firm-leaders-str='", "'"),
        ("location", "<h2>", "<"),
        ("year", "<div class=\"title\">YEAR</div>", "</"),
        ("size", "<div class=\"title\">SIZE</div>", "</"),
        ("status", "<div class=\"title\">STATUS</div>", "</"),
    ] {
        if let Some(value) = between(body,start,end).and_then(clean) {
            result.insert(key.into(), value.into());
        }
    }
    if let Some(gid) = between(body,"data-gid='","'").and_then(|v| v.rsplit('.').next()).filter(|v| slug(v)) {
        result.insert("gid".into(), gid.into());
    }
    result
}

fn trusted_original(raw: &str) -> Option<(String, String)> {
    let candidate = raw.split('?').next()?;
    let lower = candidate.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "/../", "/./"].iter().any(|v| lower.contains(v)) {
        return None;
    }
    let url = Url::parse(candidate).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !(host == "architizer.com" || host.ends_with(".architizer.com")
            || host == "architizer-prod.imgix.net")
        || url.port().is_some() || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let name = url.path_segments()?.next_back()?;
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif") { return None; }
    let filename = safe_filename(name);
    if filename.is_empty() || filename == "." || filename == ".." { return None; }
    Some((url.to_string(), filename))
}

fn originals(html: &str, project: &str)
    -> Result<Vec<(MediaItem, Metadata)>, Box<dyn Error + Send + Sync>> {
    let meta = project_meta(html);
    let mut seen=HashSet::new();
    let mut result=Vec::new();
    for chunk in html.split("property=\"og:image:secure_url\" content=\"").skip(1).take(MAX_MEDIA+1) {
        let Some(src) = chunk.split_once('"').map(|(s,_)|s) else { continue };
        let Some((url,filename)) = trusted_original(src) else { continue };
        if !seen.insert(url.clone()) { continue; }
        if result.len() >= MAX_MEDIA { return Err("Architizer media exceeds project cap".into()); }
        let ordinal=result.len()+1;
        let item=MediaItem {url,filename,extractor:"architizer",post_id:None};
        let mut fields=media_metadata(&item);
        for (key,value) in &meta { fields.insert(key.clone(),value.clone()); }
        fields.insert("subcategory".into(),"project".into());
        fields.insert("project_slug".into(),project.into());
        fields.insert("num".into(),ordinal.into());
        fields.insert("post_url".into(),format!("https://architizer.com/projects/{project}/").into());
        result.push((item,fields));
    }
    if result.is_empty() { return Err("Architizer project has no supported public original".into()); }
    Ok(result)
}

fn project_queues(html: &str, firm: &str) -> Vec<ExtractorMessage> {
    let mut seen=HashSet::new();
    let mut result=Vec::new();
    for chunk in html.split("<a href=\"/projects/").skip(1).take(3200) {
        if result.len()>=MAX_CHILDREN { break; }
        let Some(path)=chunk.split_once('"').map(|(s,_)|s.trim_end_matches('/')) else { continue };
        if !slug(path) || !seen.insert(path.to_owned()) { continue; }
        let mut fields=Metadata::new();
        fields.insert("category".into(),"architizer".into());
        fields.insert("subcategory".into(),"firm".into());
        fields.insert("firm".into(),firm.into());
        fields.insert("project_slug".into(),path.into());
        result.push(queued_url(format!("https://architizer.com/projects/{path}/"),fields));
    }
    result
}

async fn bounded_html(client: &Client, url: Url) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut response=client.get(url.clone()).send().await?.error_for_status()?;
    if response.url().origin()!=url.origin() { return Err("Architizer source redirected outside origin".into()); }
    if response.content_length().is_some_and(|n|n>MAX_HTML as u64) {
        return Err("Architizer HTML exceeds limit".into());
    }
    let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await? {
        if chunk.len()>MAX_HTML.saturating_sub(bytes.len()) { return Err("Architizer HTML exceeds limit".into()); }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn project_at(client:&Client,base:&Url,project:&str)
    -> Result<Vec<(MediaItem,Metadata)>,Box<dyn Error+Send+Sync>> {
    let url=base.join(&format!("projects/{project}/"))?;
    originals(&bounded_html(client,url).await?,project)
}

async fn firm_at(client:&Client,base:&Url,firm:&str)
    -> Result<Vec<ExtractorMessage>,Box<dyn Error+Send+Sync>> {
    let url=base.join(&format!("firms/{firm}/?requesting_merlin=pages"))?;
    Ok(project_queues(&bounded_html(client,url).await?,firm))
}

pub async fn resolve(client:&Client,target:Target)
    -> Result<Vec<MediaItem>,Box<dyn Error+Send+Sync>> {
    let base=Url::parse("https://architizer.com/")?;
    match target {
        Target::Project(project)=>Ok(project_at(client,&base,&project).await?.into_iter().map(|(item,_)|item).collect()),
        Target::Firm(_)=>Err("Architizer firm feeds require --dump-messages or --follow-queues".into()),
    }
}

pub async fn resolve_messages(client:&Client,target:Target)
    -> Result<Vec<ExtractorMessage>,Box<dyn Error+Send+Sync>> {
    let base=Url::parse("https://architizer.com/")?;
    match target {
        Target::Project(project)=>{
            let mut events=Vec::new();
            for (item,metadata) in project_at(client,&base,&project).await? {
                events.push(ExtractorMessage::Directory{metadata:metadata.clone()});
                events.push(ExtractorMessage::Url{url:item.url,metadata});
            }
            Ok(events)
        }
        Target::Firm(firm)=>firm_at(client,&base,&firm).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::TcpListener};
    const FIXTURE:&str=r#"<div id="Pages" data-name='Urban House' data-slug='urban-house' data-gid='foo.13579' data-firm-leaders-str='Studio X'><h2>Barcelona</h2><meta property="og:image:secure_url" content="https://images.architizer.com/house/first.jpg?width=1000"><meta property="og:image:secure_url" content="https://architizer-prod.imgix.net/house/second.png?fit=crop"><meta property="og:image:secure_url" content="https://evil-architizer.com/house/evil.png">"#;

    #[test]
    fn strict_public_project_and_firm_routes() {
        assert_eq!(match_url("https://architizer.com/projects/urban-house/"),Some(Target::Project("urban-house".into())));
        assert_eq!(match_url("architizer.com/firms/studio-x"),Some(Target::Firm("studio-x".into())));
        for bad in ["https://fakearchitizer.com/projects/name","https://architizer.com/projects/name?secret=x",
            "https://architizer.com:8443/firms/foo","https://architizer.com/projects/../../admin",
            "https://user:password@architizer.com/projects/name"] {
            assert!(match_url(bad).is_none(),"{bad}");
        }
    }

    #[test]
    fn canonical_project_images_and_visible_metadata() {
        let items=originals(FIXTURE,"urban-house").unwrap();
        assert_eq!(items.len(),2);
        assert_eq!(items[0].0.filename,"first.jpg");
        assert_eq!(items[1].0.filename,"second.png");
        assert_eq!(items[0].1["title"],"Urban House");
        assert_eq!(items[0].1["firm"],"Studio X");
        assert_eq!(items[0].1["gid"],"13579");
        assert_eq!(items[0].1["location"],"Barcelona");
    }

    #[test]
    fn firm_queues_deduplicate_project_slugs() {
        let html=r#"<a href="/projects/a/">A</a><a href="/projects/a/">Dup</a><a href="/projects/q/filtered">Skip</a><a href="/projects/b">B</a><a href="/projects/../../secret">Skip</a>"#;
        let events=project_queues(html,"studio-x");
        assert_eq!(events.len(),2);
        assert_eq!(serde_json::to_value(&events[0]).unwrap()["url"],"https://architizer.com/projects/a/");
        assert_eq!(serde_json::to_value(&events[1]).unwrap()["metadata"]["firm"],"studio-x");
    }

    #[tokio::test]
    async fn project_html_and_firm_html_are_bounded_loopback_requests() {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            for (path,body) in [("/projects/urban-house/",FIXTURE),
                ("/firms/studio-x/?requesting_merlin=pages",r#"<a href="/projects/urban-house/">House</a>"#)] {
                let (mut socket,_)=listener.accept().await.unwrap();
                let mut request=[0u8;4096];
                let n=socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n]).starts_with(&format!("GET {path} HTTP/1.1")));
                let response=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let base=Url::parse(&format!("http://{address}/")).unwrap();
        assert_eq!(project_at(&Client::new(),&base,"urban-house").await.unwrap().len(),2);
        assert_eq!(firm_at(&Client::new(),&base,"studio-x").await.unwrap().len(),1);
        server.await.unwrap();
    }
}
