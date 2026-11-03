//! A deliberately bounded, independently implemented subset of gallery-dl's
//! configuration overlay semantics. Only explicit files are loaded in preview.
use crate::pacing::{parse_byte_size, DelayRange};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::StatusCode;
use serde_json::{Map, Value};
use std::{error::Error, fs, path::Path, time::Duration};

#[derive(Debug, Clone, Copy)]
pub enum Format {
    Json,
    Yaml,
    Toml,
}

impl Format {
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("yaml" | "yml") => Self::Yaml,
            Some("toml") => Self::Toml,
            _ => Self::Json,
        }
    }
}

#[derive(Debug, Default)]
pub struct Config {
    root: Value,
}

#[derive(Debug, Default)]
pub struct ExtractorHttpHeaders {
    pub values: HeaderMap,
    pub disable_user_agent: bool,
}

fn overlay(target: &mut Value, overlay: Value) {
    if let (Some(destination), Value::Object(source)) = (target.as_object_mut(), &overlay) {
        for (key, value) in source {
            overlay_field(destination, key, value.clone());
        }
    } else {
        *target = overlay;
    }
}

fn overlay_field(destination: &mut Map<String, Value>, name: &str, value: Value) {
    let existing = destination.entry(name).or_insert(Value::Null);
    overlay(existing, value);
}

impl Config {
    /// Optional global HTTP timeout in seconds.
    pub fn global_http_timeout(&self) -> Result<Option<Duration>, &'static str> {
        parse_timeout(self.root.get("timeout"))
    }

    /// Optional media-downloader timeout in seconds.
    pub fn downloader_http_timeout(&self) -> Result<Option<Duration>, &'static str> {
        parse_timeout(self.root.pointer("/downloader/http/timeout"))
    }

    /// Optional media-downloader TLS certificate verification setting.
    pub fn downloader_http_verify(&self) -> Result<Option<HttpTlsVerification>, &'static str> {
        parse_http_tls_verification(self.root.pointer("/downloader/http/verify"))
    }

    /// Optional extractor-wide or category-specific TLS verification setting.
    pub fn extractor_http_verify(
        &self,
        extractor: &str,
    ) -> Result<Option<HttpTlsVerification>, &'static str> {
        let value = self
            .path(&["extractor", extractor, "verify"])
            .or_else(|| self.path(&["extractor", "verify"]));
        parse_http_tls_verification(value)
    }

    /// Optional extractor-wide or category-specific client source IP address.
    pub fn extractor_http_source_address(
        &self,
        extractor: &str,
    ) -> Result<Option<std::net::IpAddr>, &'static str> {
        let Some(value) = self
            .path(&["extractor", extractor, "source-address"])
            .or_else(|| self.path(&["extractor", "source-address"]))
        else {
            return Ok(None);
        };
        let address = value
            .as_str()
            .ok_or("extractor source-address must be an IP literal string")?
            .parse()
            .map_err(|_| "extractor source-address must be an IP literal string")?;
        Ok(Some(address))
    }

    /// Optional extractor-wide or category-specific User-Agent string.
    pub fn extractor_http_user_agent(
        &self,
        extractor: &str,
    ) -> Result<Option<String>, &'static str> {
        let Some(value) = self
            .path(&["extractor", extractor, "user-agent"])
            .or_else(|| self.path(&["extractor", "user-agent"]))
        else {
            return Ok(None);
        };
        value
            .as_str()
            .map(str::to_owned)
            .map(Some)
            .ok_or("extractor user-agent must be a string")
    }

    /// Whether bounded response-page diagnostics are enabled for an extractor.
    pub fn extractor_write_pages(&self, extractor: &str) -> Result<bool, &'static str> {
        self.path(&["extractor", extractor, "write-pages"])
            .or_else(|| self.path(&["extractor", "write-pages"]))
            .map(|value| {
                value
                    .as_bool()
                    .ok_or("extractor write-pages must be a boolean")
            })
            .transpose()
            .map(|enabled| enabled.unwrap_or(false))
    }

    /// Optional shared proxy map for extractor and media HTTP clients.
    pub fn http_proxy_settings(&self) -> Result<Option<HttpProxySettings>, &'static str> {
        self.root
            .get("proxy")
            .map(parse_proxy_settings)
            .transpose()
            .map(Option::flatten)
    }

    /// Optional downloader proxy setting; an explicitly empty value clears the shared proxy.
    pub fn downloader_http_proxy_settings(
        &self,
    ) -> Result<Option<Option<HttpProxySettings>>, &'static str> {
        self.root
            .pointer("/downloader/http/proxy")
            .map(parse_proxy_settings)
            .transpose()
    }

    /// Optional extractor-wide or category-specific proxy override.
    pub fn extractor_http_proxy_settings(
        &self,
        extractor: &str,
    ) -> Result<Option<Option<HttpProxySettings>>, &'static str> {
        self.path(&["extractor", extractor, "proxy"])
            .or_else(|| self.path(&["extractor", "proxy"]))
            .map(parse_proxy_settings)
            .transpose()
    }

    /// Optional shared or category-specific headers for extractor requests.
    pub fn extractor_http_headers(
        &self,
        extractor: &str,
    ) -> Result<Option<ExtractorHttpHeaders>, &'static str> {
        let Some(value) = self
            .path(&["extractor", extractor, "headers"])
            .or_else(|| self.path(&["extractor", "headers"]))
        else {
            return Ok(None);
        };
        let object = value
            .as_object()
            .ok_or("extractor HTTP headers must be an object")?;
        let mut headers = ExtractorHttpHeaders::default();
        for (name, value) in object {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| "invalid extractor HTTP header name")?;
            if value.is_null() {
                if name == reqwest::header::USER_AGENT {
                    headers.disable_user_agent = true;
                }
                continue;
            }
            let value = value
                .as_str()
                .ok_or("extractor HTTP header values must be strings or null")?;
            let mut value =
                HeaderValue::from_str(value).map_err(|_| "invalid extractor HTTP header value")?;
            value.set_sensitive(true);
            headers.values.insert(name, value);
        }
        Ok(Some(headers))
    }

    /// Optional bounded retry count for the media downloader.
    pub fn downloader_http_retries(&self) -> Result<Option<u32>, &'static str> {
        let Some(value) = self
            .root
            .get("retries")
            .or_else(|| self.root.pointer("/downloader/http/retries"))
        else {
            return Ok(None);
        };
        value
            .as_u64()
            .and_then(|retries| u32::try_from(retries).ok())
            .map(Some)
            .ok_or("downloader.http.retries must be a nonnegative 32-bit integer")
    }

    /// Optional source modification-time behavior for completed media files.
    pub fn downloader_http_mtime(&self) -> Result<Option<bool>, &'static str> {
        let Some(value) = self
            .root
            .get("mtime")
            .or_else(|| self.root.pointer("/downloader/http/mtime"))
        else {
            return Ok(None);
        };
        value
            .as_bool()
            .map(Some)
            .ok_or("downloader.http.mtime must be a boolean")
    }

    /// Optional minimum and maximum media sizes from downloader configuration.
    pub fn downloader_filesize_limits(&self) -> Result<(Option<u64>, Option<u64>), &'static str> {
        let minimum = parse_file_size_limit(
            self.root
                .get("filesize-min")
                .or_else(|| self.root.pointer("/downloader/http/filesize-min")),
        )?;
        let maximum = parse_file_size_limit(
            self.root
                .get("filesize-max")
                .or_else(|| self.root.pointer("/downloader/http/filesize-max")),
        )?;
        Ok((minimum, maximum))
    }

    /// Optional HTTP response chunk size, expressed as a positive byte count.
    pub fn downloader_http_chunk_size(&self) -> Result<Option<usize>, &'static str> {
        let Some(value) = self
            .root
            .get("chunk-size")
            .or_else(|| self.root.pointer("/downloader/http/chunk-size"))
        else {
            return Ok(None);
        };
        let size = match value {
            Value::Number(number) => parse_byte_size(&number.to_string())?,
            Value::String(size) => parse_byte_size(size)?,
            _ => return Err("downloader.http.chunk-size must be a positive byte size"),
        };
        if size == 0 {
            return Err("downloader.http.chunk-size must be a positive byte size");
        }
        usize::try_from(size)
            .map(Some)
            .map_err(|_| "downloader.http.chunk-size is out of range")
    }

    /// Optional HTTP HTML response validation toggle.
    pub fn downloader_http_validate_html(&self) -> Result<Option<bool>, &'static str> {
        self.root
            .get("validate-html")
            .or_else(|| self.root.pointer("/downloader/http/validate-html"))
            .map(|value| {
                value
                    .as_bool()
                    .map(Some)
                    .ok_or("downloader.http.validate-html must be a boolean")
            })
            .transpose()
            .map(Option::flatten)
    }

    /// Additional HTTP response codes the media downloader should retry.
    /// Server errors remain retryable independently of this setting.
    pub fn downloader_http_retry_codes(&self) -> Result<Vec<StatusCode>, &'static str> {
        let Some(value) = self.root.pointer("/downloader/http/retry-codes") else {
            return Ok(Vec::new());
        };
        let codes = value
            .as_array()
            .ok_or("downloader.http.retry-codes must be an array")?;
        codes
            .iter()
            .map(|value| {
                let code = value
                    .as_u64()
                    .and_then(|code| u16::try_from(code).ok())
                    .ok_or("downloader.http.retry-codes entries must be HTTP status integers")?;
                if !(100..=599).contains(&code) {
                    return Err("downloader.http.retry-codes entries must be valid HTTP statuses");
                }
                StatusCode::from_u16(code)
                    .map_err(|_| "downloader.http.retry-codes entries must be valid HTTP statuses")
            })
            .collect()
    }

    /// Build validated extra headers for the media downloader only.
    pub fn downloader_http_headers(&self) -> Result<HeaderMap, &'static str> {
        let Some(value) = self.root.pointer("/downloader/http/headers") else {
            return Ok(HeaderMap::new());
        };
        let object = value
            .as_object()
            .ok_or("downloader.http.headers must be an object")?;
        let mut headers = HeaderMap::new();
        for (name, value) in object {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| "invalid downloader HTTP header name")?;
            let value = value
                .as_str()
                .ok_or("downloader HTTP header values must be strings")?;
            let mut value =
                HeaderValue::from_str(value).map_err(|_| "invalid downloader HTTP header value")?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        Ok(headers)
    }

    pub fn load(
        &mut self,
        path: &Path,
        format: Format,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let source = fs::read_to_string(path)?;
        let parsed: Value = match format {
            Format::Json => serde_json::from_str(&source)?,
            Format::Yaml => serde_yaml::from_str(&source)?,
            Format::Toml => serde_json::to_value(toml::from_str::<toml::Value>(&source)?)?,
        };
        if !parsed.is_object() {
            return Err(format!("configuration must be an object: {}", path.display()).into());
        }
        overlay(&mut self.root, parsed);
        Ok(())
    }

    pub fn option(&mut self, assignment: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
        let (key, source) = assignment.split_once('=').ok_or("expected -o KEY=VALUE")?;
        if key.split('.').any(|part| part.is_empty()) {
            return Err("invalid empty component in configuration option".into());
        }
        let value = serde_json::from_str::<Value>(source)
            .unwrap_or_else(|_| Value::String(source.to_owned()));
        let mut at = &mut self.root;
        for part in key.split('.') {
            if !at.is_object() {
                *at = Value::Object(Map::new());
            }
            at = at
                .as_object_mut()
                .expect("created config object")
                .entry(part)
                .or_insert(Value::Null);
        }
        *at = value;
        Ok(())
    }

    fn path(&self, path: &[&str]) -> Option<&Value> {
        path.iter()
            .try_fold(&self.root, |v, segment| v.get(*segment))
    }

    /// Return an explicitly configured extractor option, preferring the
    /// category override over the extractor-wide value.
    pub fn extractor_option(&self, extractor: &str, name: &str) -> Option<&Value> {
        self.path(&["extractor", extractor, name])
            .or_else(|| self.path(&["extractor", name]))
    }

    pub fn filename(&self, extractor: &str) -> Option<&str> {
        self.path(&["filename"])
            .and_then(Value::as_str)
            .or_else(|| {
                self.path(&["extractor", extractor, "filename"])
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                self.path(&["extractor", "filename"])
                    .and_then(Value::as_str)
            })
    }

    pub fn base_directory(&self) -> Option<&str> {
        self.path(&["base-directory"])
            .and_then(Value::as_str)
            .or_else(|| {
                self.path(&["extractor", "base-directory"])
                    .and_then(Value::as_str)
            })
    }

    /// Resolve an extractor duration with global values overriding all
    /// extractor scopes, then category values overriding extractor defaults.
    pub fn duration(
        &self,
        extractor: &str,
        name: &str,
    ) -> Result<Option<DelayRange>, &'static str> {
        let value = self
            .path(&[name])
            .or_else(|| self.path(&["extractor", extractor, name]))
            .or_else(|| self.path(&["extractor", name]));
        value.map(DelayRange::from_value).transpose()
    }
}

fn parse_file_size_limit(value: Option<&Value>) -> Result<Option<u64>, &'static str> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let size = match value {
        Value::Number(number) => parse_byte_size(&number.to_string())?,
        Value::String(size) => parse_byte_size(size)?,
        _ => return Err("downloader file-size limits must be byte counts or size strings"),
    };
    Ok((size > 0).then_some(size))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpTlsVerification {
    System,
    Disabled,
    CaFile(std::path::PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpProxySettings {
    pub http: Option<String>,
    pub https: Option<String>,
}

impl HttpProxySettings {
    pub fn all(proxy: &str) -> Option<Self> {
        let proxy = normalize_proxy_url(proxy);
        (!proxy.is_empty()).then(|| Self {
            http: Some(proxy.clone()),
            https: Some(proxy),
        })
    }
}

fn normalize_proxy_url(proxy: &str) -> String {
    if proxy.contains("://") {
        proxy.to_owned()
    } else {
        format!("http://{}", proxy.trim_start_matches('/'))
    }
}

fn parse_proxy_settings(value: &Value) -> Result<Option<HttpProxySettings>, &'static str> {
    if value.is_null() {
        return Ok(None);
    }
    if let Some(proxy) = value.as_str() {
        let proxy = normalize_proxy_url(proxy);
        return Ok((!proxy.is_empty()).then(|| HttpProxySettings {
            http: Some(proxy.clone()),
            https: Some(proxy),
        }));
    }
    let object = value
        .as_object()
        .ok_or("proxy must be a URL string or an object with http/https URLs")?;
    let http = object
        .get("http")
        .map(|value| value.as_str().ok_or("proxy.http must be a URL string"))
        .transpose()?
        .map(normalize_proxy_url);
    let https = object
        .get("https")
        .map(|value| value.as_str().ok_or("proxy.https must be a URL string"))
        .transpose()?
        .map(normalize_proxy_url);
    Ok((http.is_some() || https.is_some()).then_some(HttpProxySettings { http, https }))
}

fn parse_http_tls_verification(
    value: Option<&Value>,
) -> Result<Option<HttpTlsVerification>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    if let Some(enabled) = value.as_bool() {
        return Ok(Some(if enabled {
            HttpTlsVerification::System
        } else {
            HttpTlsVerification::Disabled
        }));
    }
    value
        .as_str()
        .map(|path| Some(HttpTlsVerification::CaFile(path.into())))
        .ok_or("HTTP verify setting must be a boolean or CA certificate path")
}

fn parse_timeout(value: Option<&Value>) -> Result<Option<Duration>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    let seconds = value
        .as_f64()
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .ok_or("HTTP timeout must be a positive finite number of seconds")?;
    Duration::try_from_secs_f64(seconds)
        .map(Some)
        .map_err(|_| "HTTP timeout is out of range")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_downloader_header_values_are_marked_sensitive() {
        let mut config = Config::default();
        config
            .option(r#"downloader.http.headers={"X-Token":"synthetic-secret"}"#)
            .unwrap();
        let headers = config.downloader_http_headers().unwrap();
        assert!(headers["x-token"].is_sensitive());
        assert_eq!(headers["x-token"], "synthetic-secret");
    }

    #[test]
    fn extractor_option_prefers_category_override_over_global_value() {
        let mut config = Config::default();
        config.option("extractor.filename=global").unwrap();
        assert_eq!(
            config.extractor_option("directlink", "filename"),
            Some(&Value::String("global".into()))
        );
        config
            .option("extractor.directlink.filename=category")
            .unwrap();
        assert_eq!(
            config.extractor_option("directlink", "filename"),
            Some(&Value::String("category".into()))
        );
    }

    #[test]
    fn retry_codes_are_empty_by_default_and_validate_configured_statuses() {
        let mut config = Config::default();
        assert!(config.downloader_http_retry_codes().unwrap().is_empty());
        config
            .option("downloader.http.retry-codes=[429, 408]")
            .unwrap();
        assert_eq!(
            config.downloader_http_retry_codes().unwrap(),
            [StatusCode::TOO_MANY_REQUESTS, StatusCode::REQUEST_TIMEOUT]
        );
        config.option("downloader.http.retry-codes=[700]").unwrap();
        assert!(config.downloader_http_retry_codes().is_err());
    }

    #[test]
    fn downloader_retry_count_is_optional_bounded_and_typed() {
        let mut config = Config::default();
        assert_eq!(config.downloader_http_retries().unwrap(), None);
        config.option("downloader.http.retries=2").unwrap();
        assert_eq!(config.downloader_http_retries().unwrap(), Some(2));
        config.option("downloader.http.retries=-1").unwrap();
        assert!(config.downloader_http_retries().is_err());
        config.option("downloader.http.retries=1.5").unwrap();
        assert!(config.downloader_http_retries().is_err());
        config.option("downloader.http.retries=2").unwrap();
        config.option("retries=0").unwrap();
        assert_eq!(config.downloader_http_retries().unwrap(), Some(0));
    }

    #[test]
    fn downloader_mtime_is_optional_boolean_with_global_precedence() {
        let mut config = Config::default();
        assert_eq!(config.downloader_http_mtime().unwrap(), None);
        config.option("downloader.http.mtime=false").unwrap();
        assert_eq!(config.downloader_http_mtime().unwrap(), Some(false));
        config.option("mtime=true").unwrap();
        assert_eq!(config.downloader_http_mtime().unwrap(), Some(true));
        config.option("mtime=1").unwrap();
        assert!(config.downloader_http_mtime().is_err());
    }

    #[test]
    fn downloader_file_size_limits_parse_suffixes_and_allow_independent_bounds() {
        let mut config = Config::default();
        assert_eq!(config.downloader_filesize_limits().unwrap(), (None, None));
        config.option("downloader.http.filesize-min=500k").unwrap();
        config.option("downloader.http.filesize-max=2.5M").unwrap();
        assert_eq!(
            config.downloader_filesize_limits().unwrap(),
            (Some(500 * 1024), Some(2_621_440))
        );
        config.option("filesize-min=0").unwrap();
        assert_eq!(
            config.downloader_filesize_limits().unwrap(),
            (None, Some(2_621_440))
        );
        config.option("filesize-min=3M").unwrap();
        assert_eq!(
            config.downloader_filesize_limits().unwrap(),
            (Some(3 * 1024 * 1024), Some(2_621_440))
        );
        config.option("filesize-min=true").unwrap();
        assert!(config.downloader_filesize_limits().is_err());
    }

    #[test]
    fn downloader_http_chunk_size_parses_suffixes_and_rejects_invalid_values() {
        let mut config = Config::default();
        assert_eq!(config.downloader_http_chunk_size().unwrap(), None);
        config.option("downloader.http.chunk-size=64k").unwrap();
        assert_eq!(
            config.downloader_http_chunk_size().unwrap(),
            Some(64 * 1024)
        );
        config.option("downloader.http.chunk-size=0").unwrap();
        assert!(config.downloader_http_chunk_size().is_err());
        config.option("downloader.http.chunk-size=1.5").unwrap();
        assert_eq!(config.downloader_http_chunk_size().unwrap(), Some(2));
        config.option("downloader.http.chunk-size=64q").unwrap();
        assert!(config.downloader_http_chunk_size().is_err());
        config.option("downloader.http.chunk-size=true").unwrap();
        assert!(config.downloader_http_chunk_size().is_err());
        config.option("chunk-size=128k").unwrap();
        assert_eq!(
            config.downloader_http_chunk_size().unwrap(),
            Some(128 * 1024)
        );
        config.option("downloader.http.chunk-size=64q").unwrap();
        assert_eq!(
            config.downloader_http_chunk_size().unwrap(),
            Some(128 * 1024)
        );
    }

    #[test]
    fn downloader_http_validate_html_is_optional_boolean() {
        let mut config = Config::default();
        assert_eq!(config.downloader_http_validate_html().unwrap(), None);
        config
            .option("downloader.http.validate-html=false")
            .unwrap();
        assert_eq!(config.downloader_http_validate_html().unwrap(), Some(false));
        config.option("downloader.http.validate-html=true").unwrap();
        assert_eq!(config.downloader_http_validate_html().unwrap(), Some(true));
        config.option("downloader.http.validate-html=1").unwrap();
        assert!(config.downloader_http_validate_html().is_err());
        config.option("downloader.http.validate-html=true").unwrap();
        config.option("validate-html=false").unwrap();
        assert_eq!(config.downloader_http_validate_html().unwrap(), Some(false));
        config.option("downloader.http.validate-html=1").unwrap();
        assert_eq!(config.downloader_http_validate_html().unwrap(), Some(false));
    }

    #[test]
    fn http_timeout_config_supports_global_and_downloader_scope() {
        let mut config = Config::default();
        assert_eq!(config.global_http_timeout().unwrap(), None);
        assert_eq!(config.downloader_http_timeout().unwrap(), None);
        config.option("downloader.http.timeout=0.25").unwrap();
        assert_eq!(
            config.downloader_http_timeout().unwrap(),
            Some(Duration::from_millis(250))
        );
        config.option("timeout=2").unwrap();
        assert_eq!(
            config.global_http_timeout().unwrap(),
            Some(Duration::from_secs(2))
        );
        config.option("downloader.http.timeout=0").unwrap();
        assert!(config.downloader_http_timeout().is_err());
    }

    #[test]
    fn downloader_tls_verification_accepts_boolean_or_ca_file() {
        let mut config = Config::default();
        assert_eq!(config.downloader_http_verify().unwrap(), None);
        config.option("downloader.http.verify=false").unwrap();
        assert_eq!(
            config.downloader_http_verify().unwrap(),
            Some(HttpTlsVerification::Disabled)
        );
        config.option("downloader.http.verify=true").unwrap();
        assert_eq!(
            config.downloader_http_verify().unwrap(),
            Some(HttpTlsVerification::System)
        );
        config
            .option("downloader.http.verify=\"/tmp/cert.pem\"")
            .unwrap();
        assert_eq!(
            config.downloader_http_verify().unwrap(),
            Some(HttpTlsVerification::CaFile("/tmp/cert.pem".into()))
        );
        config.option("downloader.http.verify=42").unwrap();
        assert!(config.downloader_http_verify().is_err());
    }

    #[test]
    fn extractor_tls_verification_follows_category_precedence() {
        let mut config = Config::default();
        assert_eq!(config.extractor_http_verify("e621").unwrap(), None);
        config.option("extractor.verify=false").unwrap();
        config
            .option("extractor.e621.verify=\"/tmp/e621-ca.pem\"")
            .unwrap();
        assert_eq!(
            config.extractor_http_verify("e621").unwrap(),
            Some(HttpTlsVerification::CaFile("/tmp/e621-ca.pem".into()))
        );
        assert_eq!(
            config.extractor_http_verify("pixiv").unwrap(),
            Some(HttpTlsVerification::Disabled)
        );
        config.option("extractor.verify=42").unwrap();
        assert!(config.extractor_http_verify("pixiv").is_err());
    }

    #[test]
    fn extractor_source_address_follows_category_precedence() {
        let mut config = Config::default();
        config.option("extractor.source-address=127.0.0.2").unwrap();
        config
            .option("extractor.e621.source-address=127.0.0.3")
            .unwrap();
        assert_eq!(
            config.extractor_http_source_address("e621").unwrap(),
            Some("127.0.0.3".parse().unwrap())
        );
        assert_eq!(
            config.extractor_http_source_address("pixiv").unwrap(),
            Some("127.0.0.2".parse().unwrap())
        );
        config.option("extractor.source-address=not-an-ip").unwrap();
        assert!(config.extractor_http_source_address("pixiv").is_err());
    }

    #[test]
    fn extractor_user_agent_follows_category_precedence() {
        let mut config = Config::default();
        config
            .option("extractor.user-agent=shared-agent/1")
            .unwrap();
        config
            .option("extractor.e621.user-agent=category-agent/2")
            .unwrap();
        assert_eq!(
            config.extractor_http_user_agent("e621").unwrap().as_deref(),
            Some("category-agent/2")
        );
        assert_eq!(
            config
                .extractor_http_user_agent("pixiv")
                .unwrap()
                .as_deref(),
            Some("shared-agent/1")
        );
        config.option("extractor.user-agent=42").unwrap();
        assert!(config.extractor_http_user_agent("pixiv").is_err());
    }

    #[test]
    fn extractor_write_pages_follows_category_precedence() {
        let mut config = Config::default();
        config.option("extractor.write-pages=true").unwrap();
        config.option("extractor.e621.write-pages=false").unwrap();
        assert!(!config.extractor_write_pages("e621").unwrap());
        assert!(config.extractor_write_pages("pixiv").unwrap());
        config.option("extractor.write-pages=all").unwrap();
        assert!(config.extractor_write_pages("pixiv").is_err());
    }

    #[test]
    fn shared_proxy_config_accepts_single_urls_and_scheme_maps() {
        let mut config = Config::default();
        assert_eq!(config.http_proxy_settings().unwrap(), None);
        config.option("proxy=proxy.example:8080").unwrap();
        assert_eq!(
            config.http_proxy_settings().unwrap(),
            Some(HttpProxySettings {
                http: Some("http://proxy.example:8080".into()),
                https: Some("http://proxy.example:8080".into()),
            })
        );
        config
            .option(r#"proxy={"http":"http://proxy.example:8080","https":"proxy.example:8443"}"#)
            .unwrap();
        assert_eq!(
            config.http_proxy_settings().unwrap(),
            Some(HttpProxySettings {
                http: Some("http://proxy.example:8080".into()),
                https: Some("http://proxy.example:8443".into()),
            })
        );
        config
            .option(r#"downloader.http.proxy={"http":"media-proxy:8000"}"#)
            .unwrap();
        assert_eq!(
            config.downloader_http_proxy_settings().unwrap(),
            Some(Some(HttpProxySettings {
                http: Some("http://media-proxy:8000".into()),
                https: None,
            }))
        );
        config.option("downloader.http.proxy={}").unwrap();
        assert_eq!(config.downloader_http_proxy_settings().unwrap(), Some(None));
        config.option("downloader.http.proxy=null").unwrap();
        assert_eq!(config.downloader_http_proxy_settings().unwrap(), Some(None));
        config.option("proxy=42").unwrap();
        assert!(config.http_proxy_settings().is_err());
    }

    #[test]
    fn extractor_proxy_settings_override_the_shared_proxy_by_scope() {
        let mut config = Config::default();
        config.option("proxy=global-proxy:8000").unwrap();
        config
            .option("extractor.proxy=extractor-proxy:8001")
            .unwrap();
        config
            .option("extractor.e621.proxy=e621-proxy:8002")
            .unwrap();
        assert_eq!(
            config.extractor_http_proxy_settings("e621").unwrap(),
            Some(Some(HttpProxySettings::all("e621-proxy:8002").unwrap()))
        );
        assert_eq!(
            config.extractor_http_proxy_settings("pixiv").unwrap(),
            Some(Some(
                HttpProxySettings::all("extractor-proxy:8001").unwrap()
            ))
        );
        assert_eq!(
            config.extractor_http_proxy_settings("directlink").unwrap(),
            Some(Some(
                HttpProxySettings::all("extractor-proxy:8001").unwrap()
            ))
        );
        assert_eq!(
            Config::default()
                .extractor_http_proxy_settings("directlink")
                .unwrap(),
            None
        );
    }

    #[test]
    fn extractor_headers_follow_category_precedence_and_mark_values_sensitive() {
        let mut config = Config::default();
        config
            .option(r#"extractor.headers={"X-Shared":"default","X-Token":"secret"}"#)
            .unwrap();
        config
            .option(
                r#"extractor.e621.headers={"X-Category":"specific","X-Token":null,"User-Agent":null}"#,
            )
            .unwrap();

        let category_headers = config.extractor_http_headers("e621").unwrap().unwrap();
        assert_eq!(category_headers.values["x-category"], "specific");
        assert!(!category_headers.values.contains_key("x-shared"));
        assert!(!category_headers.values.contains_key("x-token"));
        assert!(category_headers.values["x-category"].is_sensitive());
        assert!(category_headers.disable_user_agent);

        let shared_headers = config.extractor_http_headers("pixiv").unwrap().unwrap();
        assert_eq!(shared_headers.values["x-shared"], "default");
        assert_eq!(shared_headers.values["x-token"], "secret");
        assert!(shared_headers.values["x-token"].is_sensitive());
        assert!(!shared_headers.disable_user_agent);
    }

    #[test]
    fn nested_and_global_filename_precedence() {
        let mut config = Config::default();
        config.option("extractor.filename={filename}").unwrap();
        config
            .option("extractor.danbooru.filename={id}.{extension}")
            .unwrap();
        assert_eq!(config.filename("directlink"), Some("{filename}"));
        assert_eq!(config.filename("danbooru"), Some("{id}.{extension}"));
        config.option("filename=GLOBAL_{filename}").unwrap();
        assert_eq!(config.filename("danbooru"), Some("GLOBAL_{filename}"));
    }

    #[test]
    fn typed_option_values_and_errors() {
        let mut config = Config::default();
        config.option("base-directory=\"pictures\"").unwrap();
        assert_eq!(config.base_directory(), Some("pictures"));
        config.option("extractor.danbooru.disabled=true").unwrap();
        assert_eq!(
            config.path(&["extractor", "danbooru", "disabled"]),
            Some(&Value::Bool(true))
        );
        assert!(config.option("invalid-no-equals").is_err());
        assert!(config.option("extractor..filename=foo").is_err());
    }

    #[test]
    fn merges_nested_options_instead_of_replacing_the_whole_extractor_tree() {
        let mut config = Config::default();
        overlay(
            &mut config.root,
            serde_json::json!({
                "extractor": {"danbooru": {"filename": "{id}.jpg", "sleep": 2}}
            }),
        );
        overlay(
            &mut config.root,
            serde_json::json!({
                "extractor": {"danbooru": {"filename": "{id}.webp"}}
            }),
        );
        assert_eq!(config.filename("danbooru"), Some("{id}.webp"));
        assert_eq!(
            config.path(&["extractor", "danbooru", "sleep"]),
            Some(&Value::from(2))
        );
    }

    #[test]
    fn duration_lookup_obeys_global_then_category_then_base_precedence() {
        let mut config = Config::default();
        config.option("extractor.sleep-skip=0.1").unwrap();
        config
            .option("extractor.directlink.sleep-skip=[0.2,0.3]")
            .unwrap();
        assert_eq!(
            config.duration("directlink", "sleep-skip").unwrap(),
            Some(DelayRange::from_value(&serde_json::json!([0.2, 0.3])).unwrap())
        );
        config.option("sleep-skip=0.4").unwrap();
        assert_eq!(
            config
                .duration("directlink", "sleep-skip")
                .unwrap()
                .unwrap(),
            DelayRange::parse("0.4").unwrap()
        );
    }
}
