//! Netscape-format cookie jar import, without browser database access.
//! Never log cookie names, values or file contents.
use reqwest::cookie::Jar;
use std::{error::Error, fs, path::Path, sync::Arc, time::{SystemTime, UNIX_EPOCH}};
use url::Url;

pub fn load_netscape_file(path: &Path) -> Result<Arc<Jar>, Box<dyn Error + Send + Sync>> {
    let content = fs::read_to_string(path)?;
    Ok(parse_netscape(&content))
}

pub fn parse_netscape(content: &str) -> Arc<Jar> {
    let jar = Arc::new(Jar::default());
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    for raw in content.lines() {
        let line = if let Some(rest) = raw.strip_prefix("#HttpOnly_") {
            rest
        } else if raw.starts_with('#') || raw.trim().is_empty() {
            continue;
        } else {
            raw
        };
        let fields: Vec<&str> = line.splitn(7, '\t').collect();
        if fields.len() != 7 {
            continue;
        }
        let [domain, subdomains, path, secure, expiration, name, value] =
            match <[&str; 7]>::try_from(fields.as_slice()) {
                Ok(fields) => fields,
                Err(_) => continue,
            };
        let (subdomains, secure) = match (subdomains, secure) {
            ("TRUE", "TRUE") => (true, true),
            ("TRUE", "FALSE") => (true, false),
            ("FALSE", "TRUE") => (false, true),
            ("FALSE", "FALSE") => (false, false),
            _ => continue,
        };
        if !path.starts_with('/') || name.is_empty() || name.contains([';', '=', '\r', '\n'])
            || value.contains([';', '\r', '\n']) {
            continue;
        }
        let host = domain.trim_start_matches('.');
        if host.is_empty() {
            continue;
        }
        let Ok(source) = Url::parse(&format!("https://{host}/")) else {
            continue;
        };
        if source.host_str() != Some(host) {
            continue;
        }
        let Ok(expires) = expiration.parse::<u64>() else {
            continue;
        };
        if expires != 0 && expires <= now {
            continue;
        }
        let mut header = format!("{name}={value}; Path={path}");
        if subdomains {
            header.push_str(&format!("; Domain=.{host}"));
        }
        if secure {
            header.push_str("; Secure");
        }
        if raw.starts_with("#HttpOnly_") {
            header.push_str("; HttpOnly");
        }
        if expires != 0 {
            header.push_str(&format!("; Max-Age={}", expires - now));
        }
        jar.add_cookie_str(&header, &source);
    }
    jar
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::cookie::CookieStore;

    #[test]
    fn netscape_file_honors_domain_and_path_scopes() {
        let content = "# Netscape HTTP Cookie File\n\
            .example.org\tTRUE\t/gallery\tFALSE\t0\tgallery_session\tsynthetic-secret\n\
            #HttpOnly_example.net\tFALSE\t/\tTRUE\t0\tlogged_in\tsynthetic-auth\n";
        let jar = parse_netscape(content);
        let allowed = jar.cookies(&Url::parse("https://cdn.example.org/gallery/post").unwrap())
            .unwrap().to_str().unwrap().to_owned();
        assert!(allowed.contains("gallery_session=synthetic-secret"));
        assert!(jar.cookies(&Url::parse("https://cdn.example.org/elsewhere").unwrap()).is_none());
        assert!(jar.cookies(&Url::parse("https://notexample.org/gallery/post").unwrap()).is_none());
        assert!(jar.cookies(&Url::parse("http://example.net/").unwrap()).is_none());
        let secure = jar.cookies(&Url::parse("https://example.net/").unwrap())
            .unwrap().to_str().unwrap().to_owned();
        assert!(secure.contains("logged_in=synthetic-auth"));
    }

    #[test]
    fn refuses_expired_malformed_or_header_injection_entries() {
        let content = "example.org\tFALSE\t/\tFALSE\t1\told\tsecret\n\
            example.org\tFALSE\t/\tFALSE\t0\tbad;name\tsecret\n\
            example.org\tFALSE\t/\tFALSE\t0\tgood\tvalue\n\
            broken-no-tabs\n";
        let jar = parse_netscape(content);
        let cookies = jar.cookies(&Url::parse("https://example.org/").unwrap())
            .unwrap().to_str().unwrap().to_owned();
        assert_eq!(cookies, "good=value");
    }
}
