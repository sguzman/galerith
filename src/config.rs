//! A deliberately bounded, independently implemented subset of gallery-dl's
//! configuration overlay semantics. Only explicit files are loaded in preview.
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
    pub fn load(&mut self, path: &Path, format: Format) -> Result<(), Box<dyn Error + Send + Sync>> {
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
        let (key, source) = assignment.split_once('=')
            .ok_or("expected -o KEY=VALUE")?;
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
            at = at.as_object_mut().expect("created config object")
                .entry(part).or_insert(Value::Null);
        }
        *at = value;
        Ok(())
    }

    fn path(&self, path: &[&str]) -> Option<&Value> {
        path.iter().try_fold(&self.root, |v, segment| v.get(*segment))
    }

    pub fn filename(&self, extractor: &str) -> Option<&str> {
        self.path(&["filename"]).and_then(Value::as_str)
            .or_else(|| self.path(&["extractor", extractor, "filename"]).and_then(Value::as_str))
            .or_else(|| self.path(&["extractor", "filename"]).and_then(Value::as_str))
    }

    pub fn base_directory(&self) -> Option<&str> {
        self.path(&["base-directory"]).and_then(Value::as_str)
            .or_else(|| self.path(&["extractor", "base-directory"]).and_then(Value::as_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_and_global_filename_precedence() {
        let mut config = Config::default();
        config.option("extractor.filename={filename}").unwrap();
        config.option("extractor.danbooru.filename={id}.{extension}").unwrap();
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
        assert_eq!(config.path(&["extractor", "danbooru", "disabled"]), Some(&Value::Bool(true)));
        assert!(config.option("invalid-no-equals").is_err());
        assert!(config.option("extractor..filename=foo").is_err());
    }

    #[test]
    fn merges_nested_options_instead_of_replacing_the_whole_extractor_tree() {
        let mut config = Config::default();
        overlay(&mut config.root, serde_json::json!({
            "extractor": {"danbooru": {"filename": "{id}.jpg", "sleep": 2}}
        }));
        overlay(&mut config.root, serde_json::json!({
            "extractor": {"danbooru": {"filename": "{id}.webp"}}
        }));
        assert_eq!(config.filename("danbooru"), Some("{id}.webp"));
        assert_eq!(config.path(&["extractor", "danbooru", "sleep"]), Some(&Value::from(2)));
    }
}
