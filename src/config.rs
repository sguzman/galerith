//! A deliberately bounded, independently implemented subset of gallery-dl's
//! configuration overlay semantics. Only explicit files are loaded in preview.
use crate::pacing::DelayRange;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::StatusCode;
use serde_json::{Map, Value};
use std::{error::Error, fs, path::Path};

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
