//! Guarded public Picarto channel-gallery JSON subset from pinned picarto.py.
use crate::{message::{media_metadata, ExtractorMessage, Metadata},safe_filename,MediaItem};
use reqwest::Client;
use serde_json::Value;
use std::{error::Error,time::Duration};
use url::Url;

const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_PAGES: u16 = 4;
const MAX_POSTS_PAGE: usize = 30;
const MAX_MEDIA: usize = 160;

#[derive(Debug,Clone,PartialEq,Eq)]
pub struct Gallery {user:String}

fn safe_part(value:&str)->bool {
    (1..=120).contains(&value.len())
        && value.bytes().all(|b|b.is_ascii_alphanumeric() || matches!(b,b'-'|b'_'))
}

pub fn match_url(raw:&str)->Option<Gallery> {
    let lower=raw.to_ascii_lowercase();
    if ["%2e","%2f","%5c","/../","/./"].iter().any(|s|lower.contains(s)){return None}
    let url=Url::parse(raw).ok()?;
    if !matches!(url.scheme(),"http"|"https")
        || url.host_str()!=Some("picarto.tv")
        || !url.username().is_empty()||url.password().is_some()
        || url.port().is_some()||url.query().is_some()||url.fragment().is_some()
        || raw.contains('%') {return None}
    let paths:Vec<_>=url.path_segments()?.collect();
    let user=match paths.as_slice() {
        [user,"gallery"]|[user,"gallery",""]=>*user,
        [user,"gallery",slug]|[user,"gallery",slug,""] if safe_part(slug)=>*user,
        _=>return None,
    };
    safe_part(user).then(||Gallery{user:user.into()})
}

fn source_string(value:&Value,max:usize)->Option<&str> {
    value.as_str().filter(|s|!s.is_empty() && s.len()<=max && !s.chars().any(char::is_control))
}

fn image_url(image:&Value)->Option<(String,String)> {
    let raw=image.get("name").and_then(|v|source_string(v,200))?;
    if raw.contains('/')||raw.contains('\\')||raw.contains('%')||raw.contains(".."){return None}
    let ext=raw.rsplit_once('.')?.1.to_ascii_lowercase();
    if !matches!(ext.as_str(),"jpg"|"jpeg"|"png"|"gif"|"webp"|"avif"){return None}
    let mut url=Url::parse("https://images.picarto.tv/").ok()?;
    url.path_segments_mut().ok()?.push("gallery").push(raw);
    Some((url.to_string(),ext))
}

fn source_id(data:&Value)->Option<String> {
    data.as_u64().map(|n|n.to_string()).or_else(||{
        data.as_str().filter(|s| (1..=64).contains(&s.len())
            && s.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-'||b==b'_'))
            .map(ToOwned::to_owned)
    })
}

fn emit_image(
    post:&Value,variant:Option<&Value>,user:&str,
    files:&mut Vec<(MediaItem,Metadata)>,
)->Result<(),Box<dyn Error+Send+Sync>> {
    let base=variant.unwrap_or(post);
    let image=base.get("default_image")
        .or_else(||post.get("default_image"))
        .ok_or("Picarto public post lacks source default_image")?;
    if image.is_null(){return Ok(())}
    let (url,extension)=image_url(image)
        .ok_or("Picarto public post original has invalid first-party image name")?;
    if files.len()>=MAX_MEDIA{return Err("Picarto media page exceeds bounded output count".into())}
    let id=post.get("id").and_then(source_id)
        .ok_or("Picarto post lacks source-backed id")?;
    let title=base.get("title").and_then(|v|source_string(v,200))
        .or_else(||post.get("title").and_then(|v|source_string(v,200)))
        .unwrap_or(&id);
    let filename=safe_filename(&format!("{id} {title}.{extension}"));
    if filename.is_empty(){return Err("Picarto output filename is empty".into())}
    let item=MediaItem{url,filename,extractor:"picarto",post_id:None};
    let mut meta=media_metadata(&item);
    meta.insert("subcategory".into(),"gallery".into());
    meta.insert("channel_name".into(),user.into());
    meta.insert("id".into(),id.into());
    meta.insert("title".into(),title.into());
    meta.insert("extension".into(),extension.into());
    meta.insert("post_url".into(),format!("https://picarto.tv/{user}/gallery").into());
    if let Some(date)=post.get("created_at").and_then(|v|source_string(v,60)){
        meta.insert("date_source".into(),date.into());
    }
    if let Some(variation)=variant {
        if let Some(vid)=variation.get("id").and_then(source_id) {
            meta.insert("variation_id".into(),vid.into());
        }
    }
    files.push((item,meta));
    Ok(())
}

fn decode_page(
    data:&Value,user:&str,files:&mut Vec<(MediaItem,Metadata)>
)->Result<usize,Box<dyn Error+Send+Sync>> {
    let posts=data.as_array().ok_or("Picarto channel API response was not an array")?;
    if posts.len()>MAX_POSTS_PAGE{return Err("Picarto API page exceeds 30 source posts".into())}
    for post in posts {
        if post.get("default_image").is_none() || post.get("default_image")==Some(&Value::Null){continue}
        emit_image(post,None,user,files)?;
        if let Some(variations)=post.get("variations").and_then(Value::as_array) {
            if variations.len()>12{return Err("Picarto post exposes too many variants".into())}
            for variation in variations {
                emit_image(post,Some(variation),user,files)?;
            }
        }
    }
    Ok(posts.len())
}

async fn read_json(client:&Client,endpoint:Url)->Result<Value,Box<dyn Error+Send+Sync>> {
    let mut response=client.get(endpoint.clone()).send().await?.error_for_status()?;
    if response.url().origin()!=endpoint.origin(){return Err("Picarto API redirected outside origin".into())}
    if response.content_length().is_some_and(|size|size>MAX_JSON as u64){
        return Err("Picarto JSON response exceeded byte cap".into());
    }
    let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await? {
        if chunk.len()>MAX_JSON.saturating_sub(bytes.len()){return Err("Picarto JSON response exceeded byte cap".into())}
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn at_api(
    client:&Client,root:&Url,user:&str,max_pages:u16
)->Result<Vec<(MediaItem,Metadata)>,Box<dyn Error+Send+Sync>> {
    let mut files=Vec::new();
    for page in 1..=max_pages.clamp(1,MAX_PAGES) {
        if page>1{tokio::time::sleep(Duration::from_secs(1)).await}
        let mut endpoint=root.join("api/channel-gallery")?;
        endpoint.query_pairs_mut()
            .append_pair("first","30")
            .append_pair("page",&page.to_string())
            .append_pair("filter_params[album_id]","")
            .append_pair("filter_params[channel_name]",user)
            .append_pair("filter_params[q]","")
            .append_pair("filter_params[visibility]","")
            .append_pair("order_by[field]","published_at")
            .append_pair("order_by[order]","DESC");
        let data=read_json(client,endpoint).await?;
        let count=decode_page(&data,user,&mut files)?;
        if count<MAX_POSTS_PAGE{break}
    }
    Ok(files)
}

pub async fn resolve(client:&Client,target:Gallery,max_pages:u16)
    ->Result<Vec<MediaItem>,Box<dyn Error+Send+Sync>> {
    Ok(at_api(client,&Url::parse("https://ptvintern.picarto.tv/")?,&target.user,max_pages).await?
        .into_iter().map(|(item,_)|item).collect())
}

pub async fn resolve_messages(client:&Client,target:Gallery,max_pages:u16)
    ->Result<Vec<ExtractorMessage>,Box<dyn Error+Send+Sync>> {
    let mut out=Vec::new();
    for (item,metadata) in at_api(client,&Url::parse("https://ptvintern.picarto.tv/")?,&target.user,max_pages).await? {
        out.push(ExtractorMessage::Directory{metadata:metadata.clone()});
        out.push(ExtractorMessage::Url{url:item.url,metadata});
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::TcpListener};

    #[test]
    fn matches_pinned_gallery_routes_not_lookalikes_or_private_queries() {
        assert_eq!(match_url("https://picarto.tv/artist/gallery").unwrap().user,"artist");
        assert_eq!(match_url("https://picarto.tv/artist/gallery/album/").unwrap().user,"artist");
        for bad in [
            "https://evilpicarto.tv/artist/gallery",
            "https://www.picarto.tv/artist/gallery",
            "https://picarto.tv/artist/gallery?token=secret",
            "https://picarto.tv/artist/gallery/../../private",
            "https://picarto.tv/artist/%2e%2e/gallery",
            "https://name:pass@picarto.tv/artist/gallery",
        ] {assert!(match_url(bad).is_none(),"{bad}");}
    }

    #[test]
    fn gallery_preserves_main_and_variation_image_metadata_with_trusted_names() {
        let posts=serde_json::json!([{
            "id":123,"title":"Landscape","created_at":"2026-05-03T09:00:00Z",
            "default_image":{"name":"first.png"},
            "variations":[{"id":5,"default_image":{"name":"alternate.jpg"}}]
        }]);
        let mut files=Vec::new();
        assert_eq!(decode_page(&posts,"artist",&mut files).unwrap(),1);
        assert_eq!(files.len(),2);
        assert_eq!(files[0].0.url,"https://images.picarto.tv/gallery/first.png");
        assert_eq!(files[0].0.filename,"123 Landscape.png");
        assert_eq!(files[1].1["variation_id"],"5");
        assert_eq!(files[0].1["date_source"],"2026-05-03T09:00:00Z");
        let hostile=serde_json::json!([{"id":123,"title":"Bad","default_image":{"name":"../secret.png"}}]);
        assert!(decode_page(&hostile,"artist",&mut Vec::new()).is_err());
    }

    #[tokio::test]
    async fn bounded_json_api_pagination_preserves_source_post_and_variation_order() {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            let first=(1..=30).map(|id|serde_json::json!({
                "id":id,"title":"Artwork","default_image":{"name":format!("{id}.jpg")}
            })).collect::<Vec<_>>();
            let second=serde_json::json!([{
                "id":31,"title":"Last","default_image":{"name":"last.jpg"},
                "variations":[{"id":1,"default_image":{"name":"last-alt.png"}}]
            }]);
            for (page,data) in [(1,Value::Array(first)),(2,second)] {
                let (mut socket,_)=listener.accept().await.unwrap();
                let mut request=[0u8;8192];
                let n=socket.read(&mut request).await.unwrap();
                let request=String::from_utf8_lossy(&request[..n]);
                assert!(request.starts_with("GET /api/channel-gallery?first=30&"));
                assert!(request.contains(&format!("page={page}")));
                let body=data.to_string();
                let reply=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let base=Url::parse(&format!("http://{address}/")).unwrap();
        let media=at_api(&Client::new(),&base,"artist",4).await.unwrap();
        assert_eq!(media.len(),32);
        assert_eq!(media[0].1["id"],"1");
        assert_eq!(media[30].0.filename,"31 Last.jpg");
        assert_eq!(media[31].1["variation_id"],"1");
        server.await.unwrap();
    }
}
