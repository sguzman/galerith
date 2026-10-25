//! Independent guarded public BBC programme gallery images and album queues.
//! Pinned source: gallery_dl/extractor/bbc.py. Live parity is not claimed.
use crate::{message::{media_metadata, queued_url, ExtractorMessage, Metadata}, MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{collections::HashSet, error::Error, time::Duration};
use url::Url;
const HTML_CAP: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_MEDIA: usize = 160;
const MAX_CHILDREN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Gallery { id: String, path: String },
    Programme { id: String, start: u16 },
}
fn safe_id(id: &str) -> bool {
    (1..=96).contains(&id.len()) &&
        id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}
fn bbc_host(host: &str) -> bool { matches!(host, "bbc.co.uk" | "www.bbc.co.uk") }
pub fn match_url(raw: &str) -> Option<Target> {
    let lower=raw.to_ascii_lowercase();
    if ["%2e","%2f","%5c","/../","/./"].iter().any(|x|lower.contains(x))
        || lower.ends_with("/..") || lower.ends_with("/.") { return None; }
    let url=Url::parse(raw).ok()?;
    if !matches!(url.scheme(),"http"|"https") || !bbc_host(url.host_str()?)
        || !url.username().is_empty() || url.password().is_some()
        || url.port().is_some() || url.fragment().is_some() { return None; }
    let parts:Vec<_>=url.path_segments()?.collect();
    match parts.as_slice() {
        ["programmes",id,"galleries"] | ["programmes",id,"galleries",""] if safe_id(id) => {
            let page=if url.query().is_some() {
                let p=url.query_pairs().collect::<Vec<_>>();
                if p.len()!=1 || p[0].0!="page" || p[0].1.is_empty()
                    || !p[0].1.bytes().all(|c|c.is_ascii_digit()) {return None;}
                p[0].1.parse::<u16>().ok()?
            } else {1};
            (1..=MAX_PAGES).contains(&page).then(||Target::Programme{id:(*id).into(),start:page})
        }
        ["programmes",id] | ["programmes",id,""] if safe_id(id) && url.query().is_none() =>
            Some(Target::Gallery{id:(*id).into(),path:format!("/programmes/{id}")}),
        ["programmes",id,child] | ["programmes",id,child,""]
            if safe_id(id) && safe_id(child) && *child!="galleries" && url.query().is_none() =>
            Some(Target::Gallery{id:(*id).into(),path:format!("/programmes/{id}/{child}")}),
        _=>None,
    }
}
fn between<'a>(text:&'a str,start:&str,end:&str)->Option<&'a str>{
    text.split_once(start)?.1.split_once(end).map(|(x,_)|x)
}
fn visible(s:&str)->Option<String>{
    (s.len()<=500 && !s.contains('<') && !s.chars().any(char::is_control))
        .then(||s.replace("&amp;","&").replace("&quot;","\"").replace("&#39;","'"))
}
fn original_image(s:&str)->Option<(Url,String)>{
    let url=if s.starts_with("//"){Url::parse(&format!("https:{s}")).ok()?}
        else {Url::parse(s).ok()?};
    if url.scheme()!="https" || url.host_str()!=Some("ichef.bbci.co.uk")
        || !url.username().is_empty() || url.password().is_some() || url.port().is_some()
        || url.query().is_some() || url.fragment().is_some()
        || !url.path().contains("/320x180_b/")
        || url.path().contains('%') || url.path().contains("..") {return None;}
    let name=url.path_segments()?.next_back()?;
    let ext=name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(ext.as_str(),"jpg"|"jpeg"|"png"|"gif"|"webp"){return None;}
    Some((Url::parse(&url.as_str().replace("/320x180_b/","/1920xn/")).ok()?,ext))
}
fn gallery_records(page:&str,id:&str,path:&str)->Result<Vec<(MediaItem,Metadata)>,Box<dyn Error+Send+Sync>>{
    let mut hierarchy=Vec::new();
    for script in page.split("<script").skip(1).take(60) {
        let Some((attrs,after))=script.split_once('>') else {continue};
        if !attrs.contains("application/ld+json"){continue;}
        let Some((raw,_))=after.split_once("</script>") else {continue};
        if let Ok(value)=serde_json::from_str::<Value>(raw) {
            if let Some(items)=value.get("itemListElement").and_then(Value::as_array) {
                let mut seen=HashSet::new();
                for item in items.iter().take(64) {
                    if let Some(name)=item.get("name").and_then(Value::as_str).and_then(visible) {
                        if seen.insert(name.clone()){hierarchy.push(name);}
                    }
                }
            }
        }
        break;
    }
    let title=between(page,"<h1>","</h1>")
        .and_then(|s|visible(s.rsplit_once("</span>").map_or(s,|(_,tail)|tail)));
    let description=between(page,"property=\"og:description\" content=\"","\"").and_then(visible);
    let mut seen=HashSet::new();
    let mut media=Vec::new();
    for element in page.split("class=\"gallery__thumbnail").skip(1).take(2000) {
        let Some((attrs,_))=element.split_once('>') else {continue};
        let Some(source)=between(attrs,"data-image-src=\"","\"") else {continue};
        let Some((url,ext))=original_image(source) else {continue};
        if !seen.insert(url.to_string()){continue;}
        if media.len()>=MAX_MEDIA{return Err("BBC gallery exceeded bounded media count".into());}
        let num=media.len()+1;
        let item=MediaItem{url:url.to_string(),filename:format!("bbc_{id}_{num:03}.{ext}"),extractor:"bbc",post_id:None};
        let mut meta=media_metadata(&item);
        meta.insert("subcategory".into(),"gallery".into());
        meta.insert("programme".into(),id.into());
        meta.insert("path".into(),serde_json::to_value(&hierarchy)?);
        meta.insert("num".into(),num.into());
        meta.insert("post_url".into(),format!("https://www.bbc.co.uk{path}").into());
        if let Some(title)=&title{meta.insert("title".into(),title.clone().into());}
        if let Some(desc)=&description{meta.insert("description".into(),desc.clone().into());}
        if let Some(t)=between(attrs,"data-gallery-title=\"","\"").and_then(visible){meta.insert("title_image".into(),t.into());}
        if let Some(t)=between(attrs,"data-gallery-synopsis=\"","\"").and_then(visible){meta.insert("synopsis".into(),t.into());}
        media.push((item,meta));
    }
    if media.is_empty(){return Err("BBC public gallery has no supported original media".into());}
    let count=media.len();
    for (_,meta) in &mut media{meta.insert("count".into(),count.into());}
    Ok(media)
}
fn gallery_child(raw:&str,id:&str)->Option<String>{
    let lower=raw.to_ascii_lowercase();
    if ["%2e","%2f","%5c","/../","/./"].iter().any(|s|lower.contains(s)){return None;}
    let url=Url::parse("https://www.bbc.co.uk/").ok()?.join(raw).ok()?;
    if !bbc_host(url.host_str()?) || url.scheme()!="https"
        || !url.username().is_empty() || url.password().is_some()
        || url.port().is_some() || url.query().is_some() || url.fragment().is_some(){return None;}
    if let Target::Gallery{id:found,path}=match_url(url.as_str())? {
        if found==id && path.trim_start_matches('/').split('/').count()==3 {
            return Some(format!("https://www.bbc.co.uk{path}"));
        }
    }
    None
}
fn listing_children(page:&str,id:&str,seen:&mut HashSet<String>,out:&mut Vec<ExtractorMessage>){
    for section in page.split("<a href=\"").skip(1).take(4000){
        if out.len()>=MAX_CHILDREN{break;}
        let Some((href,_))=section.split_once('"') else {continue};
        let Some(url)=gallery_child(href,id) else {continue};
        if !seen.insert(url.clone()){continue;}
        let mut meta=Metadata::new();
        meta.insert("category".into(),"bbc".into());
        meta.insert("subcategory".into(),"programme".into());
        meta.insert("programme".into(),id.into());
        out.push(queued_url(url,meta));
    }
}
async fn html(client:&Client,endpoint:Url,base:&Url)->Result<String,Box<dyn Error+Send+Sync>>{
    let mut response=client.get(endpoint).send().await?.error_for_status()?;
    if response.url().origin()!=base.origin(){return Err("BBC page redirected outside public origin".into());}
    if response.content_length().is_some_and(|n|n>HTML_CAP as u64){return Err("BBC public HTML exceeds byte cap".into());}
    let mut body=Vec::new();
    while let Some(chunk)=response.chunk().await?{
        if chunk.len()>HTML_CAP.saturating_sub(body.len()){return Err("BBC public HTML exceeds byte cap".into());}
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}
async fn at_base(client:&Client,base:&Url,target:&Target,max_pages:u16)->Result<Vec<ExtractorMessage>,Box<dyn Error+Send+Sync>>{
    match target {
        Target::Gallery{id,path}=>{
            let page=html(client,base.join(path.trim_start_matches('/'))?,base).await?;
            let images=gallery_records(&page,id,path)?;
            let mut out=Vec::with_capacity(images.len()*2);
            for (item,meta) in images {
                out.push(ExtractorMessage::Directory{metadata:meta.clone()});
                out.push(ExtractorMessage::Url{url:item.url,metadata:meta});
            }
            Ok(out)
        }
        Target::Programme{id,start}=>{
            let mut visited=HashSet::new();
            let mut seen=HashSet::new();
            let mut out=Vec::new();
            let end=start.saturating_add(max_pages.clamp(1,MAX_PAGES)).min(MAX_PAGES+1);
            for p in *start..end {
                if p>*start{tokio::time::sleep(Duration::from_millis(500)).await;}
                let mut page=base.join(&format!("programmes/{id}/galleries"))?;
                page.query_pairs_mut().append_pair("page",&p.to_string());
                if !visited.insert(page.to_string()){return Err("BBC pagination revisited page".into());}
                let data=html(client,page,base).await?;
                listing_children(&data,id,&mut seen,&mut out);
                if out.len()>=MAX_CHILDREN || !data.contains("rel=\"next\""){break;}
            }
            Ok(out)
        }
    }
}
pub async fn resolve(client:&Client,target:Target,max_pages:u16)->Result<Vec<MediaItem>,Box<dyn Error+Send+Sync>>{
    if matches!(target,Target::Programme{..}){
        return Err("BBC programme galleries require --dump-messages or --follow-queues".into());
    }
    let events=resolve_messages(client,target,max_pages).await?;
    let mut out=Vec::new();
    for event in events {
        if let ExtractorMessage::Url{url,metadata}=event {
            let filename=metadata.get("filename").and_then(Value::as_str)
                .ok_or("BBC original has no filename")?;
            out.push(MediaItem{url,filename:filename.into(),extractor:"bbc",post_id:None});
        }
    }
    Ok(out)
}
pub async fn resolve_messages(client:&Client,target:Target,max_pages:u16)->Result<Vec<ExtractorMessage>,Box<dyn Error+Send+Sync>>{
    at_base(client,&Url::parse("https://www.bbc.co.uk/")?,&target,max_pages).await
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::TcpListener};
    fn sample()->&'static str{
        r#"<script type="application/ld+json">{"itemListElement":[{"name":"Series"},{"name":"Season"},{"name":"Series"}]}</script><h1><span>Episode</span>A Great Gallery</h1><meta property="og:description" content="BBC Description"><div class="gallery__thumbnail" data-image-src="https://ichef.bbci.co.uk/images/ic/320x180_b/one.jpg" data-gallery-title="Photo One" data-gallery-synopsis="A synopsis"></div><div class="gallery__thumbnail" data-image-src="https://ichef.bbci.co.uk/images/ic/320x180_b/two.png"></div><div class="gallery__thumbnail" data-image-src="https://evil-ichef.bbci.co.uk/images/ic/320x180_b/bad.jpg"></div>"#
    }
    #[test]
    fn bbc_public_gallery_and_programme_matchers_reject_hostile_origins(){
        assert!(matches!(match_url("https://www.bbc.co.uk/programmes/p0abc"),Some(Target::Gallery{..})));
        assert!(matches!(match_url("https://bbc.co.uk/programmes/p0abc/extra"),Some(Target::Gallery{..})));
        assert!(matches!(match_url("https://www.bbc.co.uk/programmes/p0abc/galleries?page=2"),Some(Target::Programme{start:2,..})));
        for bad in ["https://evilbbc.co.uk/programmes/p0abc","https://www.bbc.co.uk:8443/programmes/p0abc","https://www.bbc.co.uk/programmes/p0abc/galleries?page=5","https://www.bbc.co.uk/programmes/p0abc/galleries?page=1&password=1","https://www.bbc.co.uk/programmes/p0abc/../secret","https://user:pass@www.bbc.co.uk/programmes/p0abc"]{
            assert!(match_url(bad).is_none(),"{bad}");
        }
    }
    #[test]
    fn gallery_jsonld_and_trusted_originals_keep_order_and_source_metadata(){
        let records=gallery_records(sample(),"p0abc","/programmes/p0abc").unwrap();
        assert_eq!(records.len(),2);
        assert_eq!(records[0].0.url,"https://ichef.bbci.co.uk/images/ic/1920xn/one.jpg");
        assert_eq!(records[1].0.filename,"bbc_p0abc_002.png");
        assert_eq!(records[0].1["path"],serde_json::json!(["Series","Season"]));
        assert_eq!(records[0].1["title"],"A Great Gallery");
        assert_eq!(records[0].1["title_image"],"Photo One");
        assert_eq!(records[0].1["count"],2);
        assert!(gallery_records(&sample().replace("ichef.bbci.co.uk","attacker.invalid"),"p0abc","/programmes/p0abc").is_err());
    }
    #[test]
    fn deferred_programme_children_deduplicate_without_forged_or_other_programme_urls(){
        let html=r#"<a href="https://www.bbc.co.uk/programmes/p0abc/one">One</a><a href="https://www.bbc.co.uk/programmes/p0abc/one">Dup</a><a href="https://www.bbc.co.uk/programmes/other/two">Other</a><a href="https://evil-bbc.co.uk/programmes/p0abc/three">Forged</a>"#;
        let mut seen=HashSet::new();let mut out=Vec::new();
        listing_children(html,"p0abc",&mut seen,&mut out);
        assert_eq!(out.len(),1);
        assert_eq!(serde_json::to_value(&out[0]).unwrap()["url"],"https://www.bbc.co.uk/programmes/p0abc/one");
    }
    #[tokio::test]
    async fn one_bounded_gallery_http_get_emits_source_images(){
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr=listener.local_addr().unwrap();
        let server=tokio::spawn(async move{
            let (mut socket,_)=listener.accept().await.unwrap();
            let mut buf=[0u8;4096];let n=socket.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("GET /programmes/p0abc HTTP/1.1"));
            let body=sample();let reply=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let base=Url::parse(&format!("http://{addr}/")).unwrap();
        let target=match_url("https://www.bbc.co.uk/programmes/p0abc").unwrap();
        let events=at_base(&Client::new(),&base,&target,4).await.unwrap();
        assert_eq!(events.len(),4);server.await.unwrap();
    }
    #[tokio::test]
    async fn two_programme_gallery_pages_queue_first_seen_children(){
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr=listener.local_addr().unwrap();
        let server=tokio::spawn(async move{
            for (path,body) in [
                ("/programmes/p0abc/galleries?page=1",r#"<a href="https://www.bbc.co.uk/programmes/p0abc/one">One</a><a rel="next" href="?page=2">Next</a>"#),
                ("/programmes/p0abc/galleries?page=2",r#"<a href="https://www.bbc.co.uk/programmes/p0abc/one">Dup</a><a href="https://www.bbc.co.uk/programmes/p0abc/two">Two</a>"#),
            ]{
                let (mut socket,_)=listener.accept().await.unwrap();
                let mut buf=[0u8;4096];let n=socket.read(&mut buf).await.unwrap();
                assert!(String::from_utf8_lossy(&buf[..n]).starts_with(&format!("GET {path} HTTP/1.1"));
                let reply=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base=Url::parse(&format!("http://{addr}/")).unwrap();
        let target=match_url("https://www.bbc.co.uk/programmes/p0abc/galleries").unwrap();
        let out=at_base(&Client::new(),&base,&target,4).await.unwrap();
        assert_eq!(out.len(),2);server.await.unwrap();
    }
}
