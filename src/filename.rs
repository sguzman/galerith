//! Restricted, safe subset of gallery-dl's filename formatter.
//! Unknown fields and format codes fail instead of silently claiming parity.
use crate::{safe_filename, MediaItem};
use std::{error::Error, fmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(pub String);

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl Error for FormatError {}

fn field(field: &str, item: &MediaItem) -> Result<String, FormatError> {
    let (name, spec) = field.split_once(':').unwrap_or((field, ""));
    let value = match name {
        "filename" => item.filename.clone(),
        "name" | "stem" => item
            .filename
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(&item.filename)
            .to_owned(),
        "extension" => item
            .filename
            .rsplit_once('.')
            .map(|(_, ext)| ext)
            .unwrap_or("")
            .to_owned(),
        "category" | "extractor" => item.extractor.to_owned(),
        "id" | "post_id" => item
            .post_id
            .ok_or_else(|| FormatError(format!("missing metadata field '{name}'")))?
            .to_string(),
        _ => return Err(FormatError(format!("unsupported filename field '{name}'"))),
    };
    if spec.is_empty() {
        return Ok(value);
    }
    // Small compatible subset: numeric padding, including Python's :>03 form.
    if !matches!(name, "id" | "post_id") {
        return Err(FormatError(format!(
            "unsupported formatting for field '{name}'"
        )));
    }
    let width = spec.strip_prefix('>').unwrap_or(spec);
    if width.is_empty() || width.len() > 3 || !width.bytes().all(|x| x.is_ascii_digit()) {
        return Err(FormatError(
            "unsupported numeric formatting specification".into(),
        ));
    }
    let parsed = width
        .parse::<usize>()
        .map_err(|_| FormatError("invalid numeric width".into()))?;
    if parsed > 64 {
        return Err(FormatError("filename numeric width is too large".into()));
    }
    if width.starts_with('0') {
        Ok(format!("{value:0>parsed$}"))
    } else {
        Ok(format!("{value:>parsed$}"))
    }
}

/// Render escaped braces, simple named fields, and a bounded ID-width format.
/// Never allow template text or its values to create directory components.
pub fn render(template: &str, item: &MediaItem) -> Result<String, FormatError> {
    let mut result = String::new();
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    result.push('{');
                    continue;
                }
                let mut key = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        None | Some('{') => {
                            return Err(FormatError("unclosed or nested filename field".into()))
                        }
                        Some(ch) => key.push(ch),
                    }
                }
                if key.is_empty() {
                    return Err(FormatError("empty filename field".into()));
                }
                result.push_str(&field(&key, item)?);
            }
            '}' => {
                if chars.peek() == Some(&'}') {
                    chars.next();
                    result.push('}');
                } else {
                    return Err(FormatError("unescaped closing brace".into()));
                }
            }
            c => result.push(c),
        }
    }
    // Validate the entire filename, not only the interpolated metadata.
    if result.is_empty()
        || result == "."
        || result == ".."
        || result.len() > 240
        || safe_filename(&result) != result
    {
        return Err(FormatError("unsafe output filename from template".into()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MediaItem {
        MediaItem {
            filename: "original.webp".into(),
            url: "https://example.test/hidden.webp?token=secret".into(),
            extractor: "danbooru",
            post_id: Some(42),
        }
    }

    #[test]
    fn template_basics_and_padding() {
        let item = sample();
        assert_eq!(
            render("{category}_{id:>05}.{extension}", &item).unwrap(),
            "danbooru_00042.webp"
        );
        assert_eq!(
            render("{stem}_{{proof}}.{extension}", &item).unwrap(),
            "original_{proof}.webp"
        );
        assert_eq!(render("{filename}", &item).unwrap(), "original.webp");
        assert_eq!(
            render("第{id:03}集.{extension}", &item).unwrap(),
            "第042集.webp"
        );
    }

    #[test]
    fn unknown_missing_and_invalid_format_codes_fail_explicitly() {
        let mut item = sample();
        item.post_id = None;
        assert!(render("{id}.jpg", &item).unwrap_err().0.contains("missing"));
        assert!(render("{url}", &item)
            .unwrap_err()
            .0
            .contains("unsupported"));
        assert!(render("{filename:02}", &item).is_err());
        assert!(render("{id", &item).is_err());
        assert!(render("{id:{extension}}", &item).is_err());
        assert!(render("extra}.jpg", &item).is_err());
        assert!(render("{}", &item).is_err());
    }

    #[test]
    fn cannot_escape_destination_or_create_a_directory() {
        let item = sample();
        for template in ["../{filename}", "/root/{filename}", "x\\y.jpg", "..", ""] {
            assert!(render(template, &item).is_err(), "{template}");
        }
    }
}
