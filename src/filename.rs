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

fn encode_url_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn split_field_spec(field: &str) -> Result<(&str, &str), FormatError> {
    let mut depth = 0usize;
    for (index, character) in field.char_indices() {
        match character {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => return Ok((&field[..index], &field[index + 1..])),
            _ => {}
        }
    }
    Ok((field, ""))
}

fn slice_field(value: &str, selector: &str) -> Result<String, FormatError> {
    let chars: Vec<char> = value.chars().collect();
    if selector.contains(':') {
        let parts: Vec<_> = selector.split(':').collect();
        if !(2..=3).contains(&parts.len()) {
            return Err(FormatError("unsupported filename slice".into()));
        }
        let index = |raw: &str| -> Result<Option<isize>, FormatError> {
            if raw.is_empty() {
                Ok(None)
            } else {
                raw.parse::<isize>()
                    .map(Some)
                    .map_err(|_| FormatError("invalid filename slice index".into()))
            }
        };
        let length = isize::try_from(chars.len())
            .map_err(|_| FormatError("filename field is too long to slice".into()))?;
        let start = index(parts[0])?;
        let stop = index(parts[1])?;
        let step = if parts.len() == 3 {
            index(parts[2])?.unwrap_or(1)
        } else {
            1
        };
        if step == 0 {
            return Err(FormatError("filename slice step cannot be zero".into()));
        }
        let normalize = |value: isize, negative_step: bool| {
            let value = if value < 0 {
                value.saturating_add(length)
            } else {
                value
            };
            if negative_step {
                value.clamp(-1, length.saturating_sub(1))
            } else {
                value.clamp(0, length)
            }
        };
        let negative_step = step < 0;
        let mut cursor = match start {
            Some(value) => normalize(value, negative_step),
            None if negative_step => length.saturating_sub(1),
            None => 0,
        };
        let end = match stop {
            Some(value) => normalize(value, negative_step),
            None if negative_step => -1,
            None => length,
        };
        let mut result = String::new();
        while if negative_step {
            cursor > end
        } else {
            cursor < end
        } {
            if let Some(character) = usize::try_from(cursor)
                .ok()
                .and_then(|index| chars.get(index))
            {
                result.push(*character);
            }
            let Some(next) = cursor.checked_add(step) else {
                break;
            };
            cursor = next;
        }
        return Ok(result);
    }
    let index = selector
        .parse::<isize>()
        .map_err(|_| FormatError("invalid filename field index".into()))?;
    let length = chars.len() as isize;
    let index = if index < 0 { length + index } else { index };
    usize::try_from(index)
        .ok()
        .and_then(|index| chars.get(index))
        .copied()
        .map(String::from)
        .ok_or_else(|| FormatError("filename field index out of range".into()))
}

fn field(field: &str, item: &MediaItem) -> Result<String, FormatError> {
    let (field_name, spec) = split_field_spec(field)?;
    let (name, conversion) = field_name.split_once('!').unwrap_or((field_name, ""));
    let (base_name, selector) = match name.split_once('[') {
        Some((base, suffix)) => (
            base,
            Some(
                suffix
                    .strip_suffix(']')
                    .ok_or_else(|| FormatError("invalid filename field selector".into()))?,
            ),
        ),
        None if name.contains(']') => {
            return Err(FormatError("invalid filename field selector".into()))
        }
        None => (name, None),
    };
    let value = match base_name {
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
            .ok_or_else(|| FormatError(format!("missing metadata field '{base_name}'")))?
            .to_string(),
        _ => {
            return Err(FormatError(format!(
                "unsupported filename field '{base_name}'"
            )))
        }
    };
    let value = match selector {
        Some(selector) => slice_field(&value, selector)?,
        None => value,
    };
    let value = match conversion {
        "" | "s" => value,
        "q" => encode_url_component(&value),
        "Q" => percent_encoding::percent_decode_str(&value)
            .decode_utf8()
            .map_err(|_| FormatError("URL-decoded filename field is not valid UTF-8".into()))?
            .into_owned(),
        "l" => value.to_lowercase(),
        "u" => value.to_uppercase(),
        "c" => {
            let mut chars = value.chars();
            match chars.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        }
        "C" => {
            let mut result = String::new();
            let mut word_start = true;
            for character in value.chars() {
                if character.is_alphanumeric() {
                    if word_start {
                        result.extend(character.to_uppercase());
                    } else {
                        result.extend(character.to_lowercase());
                    }
                    word_start = false;
                } else {
                    result.push(character);
                    word_start = true;
                }
            }
            result
        }
        "t" => value.trim().to_owned(),
        "W" => value.split_whitespace().collect::<Vec<_>>().join(" "),
        "n" => value.chars().count().to_string(),
        _ => {
            return Err(FormatError(format!(
                "unsupported conversion '{conversion}'"
            )))
        }
    };
    if spec.is_empty() {
        return Ok(value);
    }
    // Compatible conditional wrapper: `?prefix/suffix/` surrounds a
    // non-empty formatted value and emits nothing for an empty one.
    if let Some(wrapper) = spec.strip_prefix('?') {
        let (prefix, suffix) = wrapper
            .split_once('/')
            .and_then(|(prefix, suffix)| suffix.strip_suffix('/').map(|suffix| (prefix, suffix)))
            .ok_or_else(|| FormatError("invalid conditional filename format".into()))?;
        return if value.is_empty() {
            Ok(String::new())
        } else {
            Ok(format!("{prefix}{value}{suffix}"))
        };
    }
    // Bounded character-count formatters used to keep output names portable.
    // `Xn/suffix/` truncates and appends a suffix; `Ln/replacement/` replaces
    // values whose length is greater than n. Byte-oriented variants are not
    // implemented because their behavior depends on the host filesystem
    // encoding.
    if let Some((max_len, replacement)) = spec
        .strip_prefix('X')
        .or_else(|| spec.strip_prefix('L'))
        .and_then(|spec| {
            let (max_len, replacement) = spec.split_once('/')?;
            Some((max_len, replacement.strip_suffix('/')?))
        })
    {
        let truncate = spec.starts_with('X');
        let max_len = max_len
            .parse::<usize>()
            .ok()
            .filter(|length| (1..=240).contains(length))
            .ok_or_else(|| FormatError("invalid filename length limit".into()))?;
        let length = value.chars().count();
        if length <= max_len {
            return Ok(value);
        }
        if !truncate {
            return Ok(replacement.to_owned());
        }
        let replacement_len = replacement.chars().count();
        if replacement_len > max_len {
            return Err(FormatError(
                "filename length suffix exceeds its configured limit".into(),
            ));
        }
        return Ok(value
            .chars()
            .take(max_len - replacement_len)
            .chain(replacement.chars())
            .collect());
    }
    if spec.starts_with('X') || spec.starts_with('L') {
        return Err(FormatError("invalid filename length formatter".into()));
    }
    // Small compatible subset: numeric padding, including Python's :>03 form.
    if !matches!(base_name, "id" | "post_id") {
        return Err(FormatError(format!(
            "unsupported formatting for field '{base_name}'"
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
    fn conditionally_wraps_nonempty_values() {
        let item = sample();
        assert_eq!(
            render("{filename:?[/]/}", &item).unwrap(),
            "[original.webp]"
        );
        assert_eq!(render("x{extension:?[/]/}", &item).unwrap(), "x[webp]");
        assert_eq!(render("x{extension!u:?[/]/}", &item).unwrap(), "x[WEBP]");

        let mut empty = item;
        empty.filename = "extensionless".into();
        assert_eq!(render("x{extension:?[/]/}", &empty).unwrap(), "x");
    }

    #[test]
    fn bounded_length_formatters_truncate_or_replace_overlong_fields() {
        let mut item = sample();
        item.filename = "猫猫猫猫猫.png".into();
        assert_eq!(render("{stem:X4/---/}", &item).unwrap(), "猫---");
        assert_eq!(render("{stem:L4/long/}", &item).unwrap(), "long");
        assert_eq!(render("{stem:L20/long/}", &item).unwrap(), "猫猫猫猫猫");
        assert!(render("{stem:X3/long/}", &item).is_err());
        assert!(render("{stem:X0/../}", &item).is_err());
    }

    #[test]
    fn supports_lowercase_and_uppercase_field_conversions() {
        let item = sample();
        assert_eq!(
            render("{category!u}_{extension!l}", &item).unwrap(),
            "DANBOORU_webp"
        );
        assert_eq!(render("{filename!u}", &item).unwrap(), "ORIGINAL.WEBP");
        assert!(render("{category!x}", &item).is_err());
        assert!(render("{category!lower}", &item).is_err());
    }

    #[test]
    fn string_conversion_preserves_string_fields() {
        let item = sample();
        assert_eq!(render("{filename!s}", &item).unwrap(), "original.webp");
        assert_eq!(render("{id!s}", &item).unwrap(), "42");
    }

    #[test]
    fn named_fields_support_unicode_indices_and_slices() {
        let mut item = sample();
        item.filename = "猫original.webp".into();
        assert_eq!(render("{filename[0]}", &item).unwrap(), "猫");
        assert_eq!(render("{filename[-4:]}", &item).unwrap(), "webp");
        assert_eq!(render("{filename[1:4]!u}", &item).unwrap(), "ORI");
        assert_eq!(render("{filename[1:4:2]}", &item).unwrap(), "oi");
        assert_eq!(
            render("{filename[::-1]}", &item).unwrap(),
            "pbew.lanigiro猫"
        );
        assert_eq!(render("{filename[4:1:-1]}", &item).unwrap(), "gir");
        assert_eq!(render("x{filename[99:]}", &item).unwrap(), "x");
        assert!(render("{filename[99]}", &item).is_err());
        assert!(render("{filename[::0]}", &item).is_err());
    }

    #[test]
    fn url_component_conversions_encode_and_decode_utf8() {
        let mut item = sample();
        item.filename = "café + tea.jpg".into();
        assert_eq!(render("{stem!q}", &item).unwrap(), "caf%C3%A9%20%2B%20tea");
        item.filename = "caf%C3%A9%20tea.jpg".into();
        assert_eq!(render("{stem!Q}", &item).unwrap(), "café tea");
        item.filename = "%FF.jpg".into();
        assert!(render("{stem!Q}", &item).is_err());
    }

    #[test]
    fn capitalization_uppercases_first_character_and_lowers_the_rest() {
        let mut item = sample();
        item.filename = "élÉPHant.webp".into();
        assert_eq!(field("filename!c", &item).unwrap(), "Éléphant.webp");
        item.filename.clear();
        assert_eq!(field("filename!c", &item).unwrap(), "");
    }

    #[test]
    fn title_conversion_capitalizes_each_alphanumeric_word() {
        let mut item = sample();
        item.filename = "aUTUMN, ÉLAN vITAL.webp".into();
        assert_eq!(
            field("filename!C", &item).unwrap(),
            "Autumn, Élan Vital.Webp"
        );
    }

    #[test]
    fn length_conversion_counts_unicode_characters() {
        let mut item = sample();
        item.filename = "é猫.webp".into();
        assert_eq!(field("filename!n", &item).unwrap(), "7");
    }

    #[test]
    fn trim_conversion_removes_outer_unicode_whitespace() {
        let mut item = sample();
        item.filename = "\u{2003}original.webp\t".into();
        assert_eq!(field("filename!t", &item).unwrap(), "original.webp");
    }

    #[test]
    fn whitespace_conversion_collapses_runs_to_one_space() {
        let mut item = sample();
        item.filename = "  spring\t\n  rain.webp  ".into();
        assert_eq!(field("filename!W", &item).unwrap(), "spring rain.webp");
    }

    #[test]
    fn rejects_malformed_conditional_formatters() {
        let item = sample();
        for template in ["{filename:?}", "{filename:?prefix/suffix}", "{filename:?/}"] {
            assert!(render(template, &item).is_err(), "accepted {template:?}");
        }
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
