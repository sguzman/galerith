//! Restricted, safe subset of gallery-dl's filename formatter.
//! Unknown fields and format codes fail instead of silently claiming parity.
use crate::message::Metadata;
use crate::{safe_filename, MediaItem};
use serde_json::Value;
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
    if let Some(selector) = selector.strip_prefix('b') {
        return slice_field_bytes(value, selector);
    }
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

fn slice_field_bytes(value: &str, selector: &str) -> Result<String, FormatError> {
    let parts: Vec<_> = selector.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err(FormatError("byte selector requires a slice".into()));
    }
    let index = |raw: &str| -> Result<Option<isize>, FormatError> {
        if raw.is_empty() {
            Ok(None)
        } else {
            raw.parse::<isize>()
                .map(Some)
                .map_err(|_| FormatError("invalid filename byte-slice index".into()))
        }
    };
    let length = isize::try_from(value.len())
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
    let mut bytes = Vec::new();
    while if negative_step {
        cursor > end
    } else {
        cursor < end
    } {
        if let Some(byte) = usize::try_from(cursor)
            .ok()
            .and_then(|index| value.as_bytes().get(index))
        {
            bytes.push(*byte);
        }
        let Some(next) = cursor.checked_add(step) else {
            break;
        };
        cursor = next;
    }
    Ok(decode_utf8_ignoring_invalid(&bytes))
}

fn select_array(values: &[Value], selector: &str) -> Result<Value, FormatError> {
    let length = isize::try_from(values.len())
        .map_err(|_| FormatError("filename list is too long to select".into()))?;
    let parse_index = |raw: &str| -> Result<Option<isize>, FormatError> {
        if raw.is_empty() {
            Ok(None)
        } else {
            raw.parse::<isize>()
                .map(Some)
                .map_err(|_| FormatError("invalid filename list selector".into()))
        }
    };
    if !selector.contains(':') {
        let index = parse_index(selector)?
            .ok_or_else(|| FormatError("filename list selector requires an index".into()))?;
        let index = if index < 0 { length + index } else { index };
        return usize::try_from(index)
            .ok()
            .and_then(|index| values.get(index))
            .cloned()
            .ok_or_else(|| FormatError("filename list index out of range".into()));
    }

    let parts: Vec<_> = selector.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err(FormatError("unsupported filename list slice".into()));
    }
    let step = if parts.len() == 3 {
        parse_index(parts[2])?.unwrap_or(1)
    } else {
        1
    };
    if step == 0 {
        return Err(FormatError(
            "filename list slice step cannot be zero".into(),
        ));
    }
    let negative_step = step < 0;
    let normalize = |value: isize| {
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
    let mut cursor = match parse_index(parts[0])? {
        Some(value) => normalize(value),
        None if negative_step => length.saturating_sub(1),
        None => 0,
    };
    let end = match parse_index(parts[1])? {
        Some(value) => normalize(value),
        None if negative_step => -1,
        None => length,
    };
    let mut selected = Vec::new();
    while if negative_step {
        cursor > end
    } else {
        cursor < end
    } {
        if let Some(value) = usize::try_from(cursor)
            .ok()
            .and_then(|index| values.get(index))
        {
            selected.push(value.clone());
        }
        let Some(next) = cursor.checked_add(step) else {
            break;
        };
        cursor = next;
    }
    Ok(Value::Array(selected))
}

fn decode_utf8_ignoring_invalid(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len());
    let mut remaining = bytes;
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                result.push_str(valid);
                break;
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                // gallery-dl decodes byte slices with UTF-8's "ignore" error
                // policy: retain valid text and discard malformed byte runs.
                result.push_str(
                    std::str::from_utf8(&remaining[..valid_up_to])
                        .expect("valid_up_to marks a valid UTF-8 prefix"),
                );
                let invalid_len = error.error_len().unwrap_or(remaining.len() - valid_up_to);
                remaining = &remaining[valid_up_to + invalid_len..];
            }
        }
    }
    result
}

fn python_string_repr(value: &str, ascii_only: bool) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut result = String::with_capacity(value.len() + 2);
    result.push(quote);
    for character in value.chars() {
        match character {
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character == quote => {
                result.push('\\');
                result.push(character);
            }
            character if character.is_control() => {
                let code = u32::from(character);
                if code <= 0xff {
                    result.push_str(&format!("\\x{code:02x}"));
                } else if code <= 0xffff {
                    result.push_str(&format!("\\u{code:04x}"));
                } else {
                    result.push_str(&format!("\\U{code:08x}"));
                }
            }
            character if ascii_only && !character.is_ascii() => {
                let code = u32::from(character);
                if code <= 0xff {
                    result.push_str(&format!("\\x{code:02x}"));
                } else if code <= 0xffff {
                    result.push_str(&format!("\\u{code:04x}"));
                } else {
                    result.push_str(&format!("\\U{code:08x}"));
                }
            }
            character => result.push(character),
        }
    }
    result.push(quote);
    result
}

fn python_capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

fn python_capwords(value: &str) -> String {
    value
        .split_whitespace()
        .map(python_capitalize)
        .collect::<Vec<_>>()
        .join(" ")
}

fn python_json_string(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('"');
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{08}' => result.push_str("\\b"),
            '\u{0c}' => result.push_str("\\f"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character <= '\u{1f}' => {
                result.push_str(&format!("\\u{:04x}", u32::from(character)));
            }
            character if !character.is_ascii() => {
                let code = u32::from(character);
                if code <= 0xffff {
                    result.push_str(&format!("\\u{code:04x}"));
                } else {
                    let scalar = code - 0x1_0000;
                    let high = 0xd800 + (scalar >> 10);
                    let low = 0xdc00 + (scalar & 0x3ff);
                    result.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
                }
            }
            character => result.push(character),
        }
    }
    result.push('"');
    result
}

fn slugify_ascii(value: &str) -> Result<String, FormatError> {
    if !value.is_ascii() {
        return Err(FormatError(
            "slug conversion currently accepts ASCII values only".into(),
        ));
    }
    let mut result = String::with_capacity(value.len());
    let mut separator = false;
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() {
            if separator && !result.is_empty() {
                result.push('-');
            }
            result.push(byte.to_ascii_lowercase() as char);
            separator = false;
        } else if matches!(byte, b'.' | b':') {
            // gallery-dl's !g slugifier drops dots and colons (notably in
            // domains and ports) instead of treating them as separators.
        } else {
            separator = true;
        }
    }
    Ok(result)
}

fn format_radix_id(value: &str, spec: &str) -> Result<Option<String>, FormatError> {
    let Some((radix, _suffix)) = spec.split_at_checked(spec.len().saturating_sub(1)) else {
        return Ok(None);
    };
    let Some(kind) = spec.chars().last() else {
        return Ok(None);
    };
    let (base, marker, grouping) = match kind {
        'd' => (10, "", None),
        'b' => (2, "0b", None),
        'o' => (8, "0o", None),
        'x' => (16, "0x", None),
        ',' => (10, "", Some(',')),
        '_' => (10, "", Some('_')),
        _ => return Ok(None),
    };
    // Only the integer subset of Python's numeric mini-language needed for
    // post IDs: optional alternate prefix, zero fill, and a bounded width.
    let alternate = radix.starts_with('#');
    let width_spec = radix.strip_prefix('#').unwrap_or(radix);
    let zero_fill = width_spec.starts_with('0') && width_spec.len() > 1;
    if alternate && (grouping.is_some() || base == 10) {
        return Err(FormatError(
            "unsupported alternate numeric formatting specification".into(),
        ));
    }
    if grouping.is_some() && zero_fill {
        return Err(FormatError(
            "unsupported grouped numeric formatting specification".into(),
        ));
    }
    let width_text = if zero_fill {
        &width_spec[1..]
    } else {
        width_spec
    };
    if !width_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FormatError(
            "unsupported numeric formatting specification".into(),
        ));
    }
    let width = if width_text.is_empty() {
        0
    } else {
        width_text
            .parse::<usize>()
            .map_err(|_| FormatError("invalid numeric width".into()))?
    };
    if width > 64 {
        return Err(FormatError("filename numeric width is too large".into()));
    }
    let number = value
        .parse::<u64>()
        .map_err(|_| FormatError("numeric formatting requires an unsigned integer field".into()))?;
    let mut digits = match (base, grouping) {
        (10, Some(separator)) => {
            let raw = number.to_string();
            let mut reversed = String::with_capacity(raw.len() + raw.len() / 3);
            for (index, digit) in raw.chars().rev().enumerate() {
                if index > 0 && index % 3 == 0 {
                    reversed.push(separator);
                }
                reversed.push(digit);
            }
            reversed.chars().rev().collect()
        }
        (10, None) => number.to_string(),
        (2, None) => format!("{number:b}"),
        (8, None) => format!("{number:o}"),
        (16, None) => format!("{number:x}"),
        _ => unreachable!(),
    };
    let prefix = if alternate { marker } else { "" };
    let padding = width.saturating_sub(prefix.len() + digits.len());
    if zero_fill {
        digits.insert_str(0, &"0".repeat(padding));
        Ok(Some(format!("{prefix}{digits}")))
    } else {
        Ok(Some(format!("{}{prefix}{digits}", " ".repeat(padding))))
    }
}

fn join_formatter_spec(spec: &str) -> Option<(&str, &str)> {
    let spec = spec.strip_prefix('J')?;
    Some(spec.split_once('/').unwrap_or((spec, "")))
}

fn map_formatter_spec(spec: &str) -> Option<(&str, &str)> {
    let spec = spec.strip_prefix('M')?;
    Some(spec.split_once('/').unwrap_or((spec, "")))
}

fn sort_formatter_spec(spec: &str) -> Option<(&str, &str)> {
    let spec = spec.strip_prefix('S')?;
    Some(spec.split_once('/').unwrap_or((spec, "")))
}

fn conversion_formatter_spec(spec: &str) -> Option<(&str, &str)> {
    let spec = spec.strip_prefix('C')?;
    Some(spec.split_once('/').unwrap_or((spec, "")))
}

fn convert_string_chain(value: &str, conversions: &str) -> Result<String, FormatError> {
    if conversions.is_empty() {
        return Err(FormatError(
            "filename conversion formatter requires a conversion".into(),
        ));
    }
    let mut value = value.to_owned();
    for conversion in conversions.chars() {
        value = match conversion {
            'l' => value.to_lowercase(),
            'u' => value.to_uppercase(),
            'c' => {
                let mut chars = value.chars();
                match chars.next() {
                    Some(first) => first
                        .to_uppercase()
                        .chain(chars.flat_map(char::to_lowercase))
                        .collect(),
                    None => String::new(),
                }
            }
            'C' => python_capwords(&value),
            't' => value.trim().to_owned(),
            's' => value,
            'g' => slugify_ascii(&value)?,
            'j' => python_json_string(&value),
            'r' => python_string_repr(&value, false),
            'a' => python_string_repr(&value, true),
            'q' => encode_url_component(&value),
            'Q' => percent_encoding::percent_decode_str(&value)
                .decode_utf8()
                .map_err(|_| {
                    FormatError("URL-decoded filename conversion is not valid UTF-8".into())
                })?
                .into_owned(),
            'W' => value.split_whitespace().collect::<Vec<_>>().join(" "),
            _ => {
                return Err(FormatError(format!(
                    "unsupported filename string conversion '{conversion}'"
                )))
            }
        };
    }
    Ok(value)
}

fn metadata_field(
    value: &Value,
    field_name: &str,
    conversion: &str,
    spec: &mut &str,
) -> Result<Option<String>, FormatError> {
    match value {
        Value::Null => Ok(None),
        Value::Bool(false) if conversion.is_empty() && spec.starts_with('?') => {
            Ok(Some(String::new()))
        }
        Value::Number(number)
            if conversion.is_empty()
                && spec.starts_with('?')
                && (number.as_i64() == Some(0)
                    || number.as_u64() == Some(0)
                    || number.as_f64() == Some(0.0)) =>
        {
            Ok(Some(String::new()))
        }
        Value::String(value) if spec.starts_with('M') => {
            let (_, remainder) = map_formatter_spec(spec).expect("map formatter starts with M");
            *spec = remainder;
            metadata_field(&Value::String(value.clone()), field_name, conversion, spec)
        }
        Value::String(value) if spec.starts_with('C') => {
            let (conversions, remainder) =
                conversion_formatter_spec(spec).expect("conversion formatter starts with C");
            let converted = convert_string_chain(value, conversions)?;
            *spec = remainder;
            metadata_field(&Value::String(converted), field_name, conversion, spec)
        }
        Value::String(value) => Ok(Some(value.clone())),
        Value::Array(values) if spec.starts_with('M') => {
            let (key, remainder) = map_formatter_spec(spec).expect("map formatter starts with M");
            if key.is_empty() {
                return Err(FormatError("filename map formatter requires a key".into()));
            }
            let mapped = values
                .iter()
                .map(|value| match value {
                    Value::Object(fields) => match fields.get(key) {
                        Some(Value::String(value)) => Ok(Value::String(value.clone())),
                        Some(_) => Err(FormatError(format!(
                            "filename map field '{field_name}.{key}' must be a string"
                        ))),
                        None => Err(FormatError(format!(
                            "filename map field '{field_name}.{key}' is missing"
                        ))),
                    },
                    _ => Err(FormatError(format!(
                        "filename map field '{field_name}' must contain objects"
                    ))),
                })
                .collect::<Result<Vec<_>, _>>()?;
            *spec = remainder;
            metadata_field(&Value::Array(mapped), field_name, conversion, spec)
        }
        Value::Array(values) if spec.starts_with('S') => {
            let (options, remainder) =
                sort_formatter_spec(spec).expect("sort formatter starts with S");
            let mut sorted = values
                .iter()
                .map(|value| match value {
                    Value::String(value) => Ok(value.clone()),
                    _ => Err(FormatError(format!(
                        "filename sort field '{field_name}' must contain only strings"
                    ))),
                })
                .collect::<Result<Vec<_>, _>>()?;
            sorted.sort();
            if options.contains('d') || options.contains('r') {
                sorted.reverse();
            }
            *spec = remainder;
            metadata_field(
                &Value::Array(sorted.into_iter().map(Value::String).collect()),
                field_name,
                conversion,
                spec,
            )
        }
        Value::Array(_) if spec.starts_with('C') => Err(FormatError(
            "filename string conversions require a string field".into(),
        )),
        Value::Number(_) | Value::Bool(_) if spec.starts_with('C') => Err(FormatError(
            "filename string conversions require a string field".into(),
        )),
        Value::Array(values) if spec.starts_with('J') => {
            let (separator, remainder) =
                join_formatter_spec(spec).expect("join formatter spec starts with J");
            let values = values
                .iter()
                .map(|value| match value {
                    Value::String(value) => Ok(value.as_str()),
                    _ => Err(FormatError(format!(
                        "filename list field '{field_name}' must contain only strings"
                    ))),
                })
                .collect::<Result<Vec<_>, _>>()?;
            *spec = remainder;
            Ok(Some(values.join(separator)))
        }
        Value::Number(_) | Value::Bool(_) if spec.starts_with('J') => Err(FormatError(
            "filename join formatter requires a string or string list".into(),
        )),
        Value::Number(value) => Ok(Some(value.to_string())),
        Value::Bool(value) => Ok(Some(if *value { "True" } else { "False" }.into())),
        Value::Array(values) if conversion == "S" => values
            .iter()
            .map(|value| match value {
                Value::String(value) => Ok(value.as_str()),
                _ => Err(FormatError(format!(
                    "filename list field '{field_name}' must contain only strings"
                ))),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|values| Some(values.join(", "))),
        Value::Array(_) => Err(FormatError(format!(
            "filename list field '{field_name}' requires the !S conversion"
        ))),
        Value::Object(_) => Err(FormatError(format!(
            "nested filename field '{field_name}' is unsupported"
        ))),
    }
}

fn field_with_metadata(
    field: &str,
    item: &MediaItem,
    metadata: &Metadata,
) -> Result<String, FormatError> {
    let (field_name, spec) = split_field_spec(field)?;
    let (name, conversion) = field_name.split_once('!').unwrap_or((field_name, ""));
    let candidates: Vec<_> = name.split('|').collect();
    if candidates.iter().any(|candidate| candidate.is_empty()) {
        return Err(FormatError("empty alternative filename field".into()));
    }
    let mut value = None;
    let mut selected_base = None;
    let mut empty_fallback = None;
    let mut selected_spec = None;
    let mut missing_field = None;
    for candidate in &candidates {
        let mut candidate_spec = spec;
        let (base_name, selector) = match candidate.split_once('[') {
            Some((base, suffix)) => (
                base,
                Some(
                    suffix
                        .strip_suffix(']')
                        .ok_or_else(|| FormatError("invalid filename field selector".into()))?,
                ),
            ),
            None if candidate.contains(']') => {
                return Err(FormatError("invalid filename field selector".into()))
            }
            None => (*candidate, None),
        };
        let mut selector_applied_to_list = false;
        let (field_value, is_literal) = if base_name == "_lit" {
            let literal = selector.ok_or_else(|| {
                FormatError("literal filename field requires a bracketed value".into())
            })?;
            if literal.is_empty() {
                return Err(FormatError("literal filename field cannot be empty".into()));
            }
            (Some(literal.to_owned()), true)
        } else {
            (
                match base_name {
                    "filename" | "name" | "stem" => Some(
                        item.filename
                            .rsplit_once('.')
                            .map(|(stem, _)| stem)
                            .unwrap_or(&item.filename)
                            .to_owned(),
                    ),
                    "extension" => Some(
                        item.filename
                            .rsplit_once('.')
                            .map(|(_, ext)| ext)
                            .unwrap_or("")
                            .to_owned(),
                    ),
                    "category" | "extractor" => Some(item.extractor.to_owned()),
                    "id" | "post_id" => match item.post_id {
                        Some(id) => Some(id.to_string()),
                        None => {
                            missing_field.get_or_insert(base_name);
                            None
                        }
                    },
                    _ => match metadata.get(base_name) {
                        Some(value) => {
                            let selected = match (value, selector) {
                                (Value::Array(values), Some(selector)) => {
                                    Some(select_array(values, selector)?)
                                }
                                _ => None,
                            };
                            selector_applied_to_list = selected.is_some();
                            let value = selected.as_ref().unwrap_or(value);
                            let value =
                                metadata_field(value, base_name, conversion, &mut candidate_spec)?;
                            if value.is_none() {
                                missing_field.get_or_insert(base_name);
                            }
                            value
                        }
                        None => {
                            if metadata.is_empty() {
                                return Err(FormatError(format!(
                                    "unsupported filename field '{base_name}'"
                                )));
                            }
                            missing_field.get_or_insert(base_name);
                            None
                        }
                    },
                },
                false,
            )
        };
        let Some(field_value) = field_value else {
            continue;
        };
        if let Some((_, remainder)) = join_formatter_spec(candidate_spec) {
            if matches!(base_name, "id" | "post_id") {
                return Err(FormatError(
                    "filename join formatter requires a string or string list".into(),
                ));
            }
            candidate_spec = remainder;
        }
        let field_value = match (is_literal, selector_applied_to_list, selector) {
            (true, _, _) | (false, true, _) | (false, false, None) => field_value,
            (false, false, Some(selector)) => slice_field(&field_value, selector)?,
        };
        if !field_value.is_empty() && value.is_none() {
            selected_base = Some(base_name);
            selected_spec = Some(candidate_spec);
            value = Some(field_value);
        } else if field_value.is_empty() && value.is_none() {
            empty_fallback.get_or_insert((base_name, field_value, candidate_spec));
        }
    }
    let (base_name, value) = match (selected_base, value, empty_fallback) {
        (Some(base), Some(value), _) => (base, value),
        (None, None, Some((base, value, spec))) => {
            selected_spec = Some(spec);
            (base, value)
        }
        (None, None, None) if candidates.len() == 1 => {
            if let Some(missing) = missing_field {
                return Err(FormatError(format!("missing metadata field '{missing}'")));
            }
            (
                candidates[0].split('[').next().unwrap_or(candidates[0]),
                String::new(),
            )
        }
        (None, None, None) => (
            candidates[0].split('[').next().unwrap_or(candidates[0]),
            String::new(),
        ),
        _ => unreachable!("selected filename field must have a value"),
    };
    let mut spec = selected_spec.unwrap_or(spec);
    let mut value = match conversion {
        "" | "s" | "S" => value,
        "r" if matches!(base_name, "id" | "post_id") => value,
        "r" => python_string_repr(&value, false),
        "a" if matches!(base_name, "id" | "post_id") => value,
        "a" => python_string_repr(&value, true),
        "j" => python_json_string(&value),
        "g" => slugify_ascii(&value)?,
        "q" => encode_url_component(&value),
        "Q" => percent_encoding::percent_decode_str(&value)
            .decode_utf8()
            .map_err(|_| FormatError("URL-decoded filename field is not valid UTF-8".into()))?
            .into_owned(),
        "l" => value.to_lowercase(),
        "u" => value.to_uppercase(),
        "c" => python_capitalize(&value),
        "C" => python_capwords(&value),
        "t" => value.trim().to_owned(),
        "W" => value.split_whitespace().collect::<Vec<_>>().join(" "),
        "n" => {
            return Err(FormatError(
                "length conversion produces a non-string filename value".into(),
            ))
        }
        _ => {
            return Err(FormatError(format!(
                "unsupported conversion '{conversion}'"
            )))
        }
    };
    // gallery-dl's identity formatter returns its input directly, ignoring
    // any remaining specifier text.
    if spec.starts_with('I') {
        return Ok(value);
    }
    while let Some(operation) = spec.strip_prefix('R') {
        let (old, remainder) = operation
            .split_once('/')
            .ok_or_else(|| FormatError("invalid filename replacement formatter".into()))?;
        let (new, remainder) = remainder
            .split_once('/')
            .ok_or_else(|| FormatError("invalid filename replacement formatter".into()))?;
        let occurrences = if old.is_empty() {
            value.chars().count().saturating_add(1)
        } else {
            value.matches(old).count()
        };
        let output_len = value
            .len()
            .checked_sub(old.len().saturating_mul(occurrences))
            .and_then(|length| length.checked_add(new.len().checked_mul(occurrences)?))
            .ok_or_else(|| FormatError("filename replacement output is too long".into()))?;
        if output_len > 240 {
            return Err(FormatError(
                "filename replacement output exceeds 240 bytes".into(),
            ));
        }
        value = value.replace(old, new);
        spec = remainder;
    }
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
    // Bounded character-count and UTF-8 byte-count formatters used to keep
    // output names portable. Byte-oriented truncation ignores an incomplete
    // final code point, matching the formatter's UTF-8 decoding behavior.
    if let Some((max_len, replacement)) = spec
        .strip_prefix('X')
        .or_else(|| spec.strip_prefix('L'))
        .and_then(|spec| {
            let (max_len, replacement) = spec.split_once('/')?;
            Some((max_len, replacement.strip_suffix('/')?))
        })
    {
        let truncate = spec.starts_with('X');
        let byte_len = max_len.starts_with('b');
        let max_len = max_len
            .strip_prefix('b')
            .unwrap_or(max_len)
            .parse::<usize>()
            .ok()
            .filter(|length| (1..=240).contains(length))
            .ok_or_else(|| FormatError("invalid filename length limit".into()))?;
        let length = if byte_len {
            value.len()
        } else {
            value.chars().count()
        };
        if length <= max_len {
            return Ok(value);
        }
        if !truncate {
            return Ok(replacement.to_owned());
        }
        let replacement_len = if byte_len {
            replacement.len()
        } else {
            replacement.chars().count()
        };
        if replacement_len > max_len {
            return Err(FormatError(
                "filename length suffix exceeds its configured limit".into(),
            ));
        }
        if byte_len {
            let prefix =
                decode_utf8_ignoring_invalid(&value.as_bytes()[..max_len - replacement_len]);
            return Ok(format!("{prefix}{replacement}"));
        }
        return Ok(value
            .chars()
            .take(max_len - replacement_len)
            .chain(replacement.chars())
            .collect());
    }
    if matches!(base_name, "id" | "post_id") {
        if let Some(formatted) = format_radix_id(&value, spec)? {
            return Ok(formatted);
        }
    }
    if spec.starts_with('X') || spec.starts_with('L') {
        return Err(FormatError("invalid filename length formatter".into()));
    }
    if let Some(operation) = spec.strip_prefix('A') {
        let operation = operation
            .strip_suffix('/')
            .ok_or_else(|| FormatError("invalid filename arithmetic formatter".into()))?;
        let (operator, operand) = operation
            .split_at_checked(1)
            .ok_or_else(|| FormatError("invalid filename arithmetic formatter".into()))?;
        let right = operand
            .parse::<i128>()
            .map_err(|_| FormatError("invalid filename arithmetic operand".into()))?;
        let left = match value.parse::<i128>() {
            Ok(left) => left,
            Err(_) if operator == "*" => {
                let count = usize::try_from(right.max(0))
                    .map_err(|_| FormatError("filename repetition count is too large".into()))?;
                let output_len = value
                    .len()
                    .checked_mul(count)
                    .filter(|length| *length <= 240)
                    .ok_or_else(|| FormatError("filename repetition exceeds 240 bytes".into()))?;
                let mut repeated = String::with_capacity(output_len);
                for _ in 0..count {
                    repeated.push_str(&value);
                }
                return Ok(repeated);
            }
            Err(_) => {
                return Err(FormatError(
                    "filename arithmetic requires an integer field".into(),
                ))
            }
        };
        let result = match operator {
            "+" => left.checked_add(right),
            "-" => left.checked_sub(right),
            "*" => left.checked_mul(right),
            _ => {
                return Err(FormatError(
                    "unsupported filename arithmetic operator".into(),
                ))
            }
        }
        .ok_or_else(|| FormatError("filename arithmetic overflow".into()))?;
        return Ok(result.to_string());
    }
    if let Some(precision) = spec.strip_prefix('.') {
        if matches!(base_name, "id" | "post_id")
            || precision.is_empty()
            || !precision.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(FormatError("invalid string precision formatting".into()));
        }
        let precision = precision
            .parse::<usize>()
            .map_err(|_| FormatError("invalid string precision".into()))?;
        if precision > 240 {
            return Err(FormatError("filename string precision is too large".into()));
        }
        return Ok(value.chars().take(precision).collect());
    }
    // Bounded Python-style width formatting for numeric IDs and strings.
    if matches!(base_name, "id" | "post_id") {
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
        return if width.starts_with('0') {
            Ok(format!("{value:0>parsed$}"))
        } else {
            Ok(format!("{value:>parsed$}"))
        };
    }
    let chars: Vec<char> = spec.chars().collect();
    let (fill, align, width_start) = match chars.as_slice() {
        [align, ..] if matches!(align, '<' | '>' | '^') => (' ', *align, 1),
        [fill, align, ..] if matches!(align, '<' | '>' | '^') => (*fill, *align, 2),
        _ => {
            return Err(FormatError(format!(
                "unsupported formatting for field '{base_name}'"
            )))
        }
    };
    let width_text: String = chars[width_start..].iter().collect();
    if width_text.is_empty() || !width_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FormatError("invalid string width formatting".into()));
    }
    let width = width_text
        .parse::<usize>()
        .map_err(|_| FormatError("invalid string width".into()))?;
    if width > 240 {
        return Err(FormatError("filename string width is too large".into()));
    }
    let value_len = value.chars().count();
    let padding = width.saturating_sub(value_len);
    let (left, right) = match align {
        '<' => (0, padding),
        '>' => (padding, 0),
        '^' => (padding / 2, padding - padding / 2),
        _ => unreachable!(),
    };
    Ok(std::iter::repeat_n(fill, left)
        .chain(value.chars())
        .chain(std::iter::repeat_n(fill, right))
        .collect())
}

#[cfg(test)]
fn field(field: &str, item: &MediaItem) -> Result<String, FormatError> {
    field_with_metadata(field, item, &Metadata::new())
}

/// Render escaped braces, simple named fields, and a bounded ID-width format.
/// Never allow template text or its values to create directory components.
pub fn render(template: &str, item: &MediaItem) -> Result<String, FormatError> {
    render_with_metadata(template, item, &Metadata::new())
}

/// Render a filename template using source URL-event metadata for additional
/// scalar fields and string lists with the !S human-readable conversion.
pub fn render_with_metadata(
    template: &str,
    item: &MediaItem,
    metadata: &Metadata,
) -> Result<String, FormatError> {
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
                result.push_str(&field_with_metadata(&key, item, metadata)?);
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
        assert_eq!(render("{filename}", &item).unwrap(), "original");
        assert_eq!(
            render("{filename}.{extension}", &item).unwrap(),
            "original.webp"
        );
        assert_eq!(
            render("第{id:03}集.{extension}", &item).unwrap(),
            "第042集.webp"
        );
        assert_eq!(render("{filename:>9}", &item).unwrap(), " original");
        assert_eq!(render("{category:_<12}", &item).unwrap(), "danbooru____");
        assert_eq!(render("{filename:#^9}", &item).unwrap(), "original#");
        assert_eq!(render("{filename:#^10}", &item).unwrap(), "#original#");
        assert_eq!(render("{category:.5}", &item).unwrap(), "danbo");
        assert_eq!(render("x{filename:.0}", &item).unwrap(), "x");
        assert_eq!(render("{_lit[proof]}", &item).unwrap(), "proof");
        assert!(render("{_lit[]}", &item).is_err());
        assert!(render("{_lit}", &item).is_err());
        assert!(render("{filename:03}", &item).is_err());
        assert!(render("{filename:>241}", &item).is_err());
    }

    #[test]
    fn conditionally_wraps_nonempty_values() {
        let item = sample();
        assert_eq!(render("{filename:?[/]/}", &item).unwrap(), "[original]");
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

        item.filename = "é猫x.jpg".into();
        assert_eq!(render("{stem:Xb4/_/}", &item).unwrap(), "é_");
        assert_eq!(render("{stem:Lb4/_/}", &item).unwrap(), "_");
        assert_eq!(render("{stem:Xb6/_/}", &item).unwrap(), "é猫x");
    }

    #[test]
    fn supports_lowercase_and_uppercase_field_conversions() {
        let item = sample();
        assert_eq!(
            render("{category!u}_{extension!l}", &item).unwrap(),
            "DANBOORU_webp"
        );
        assert_eq!(render("{filename!u}", &item).unwrap(), "ORIGINAL");
        assert!(render("{category!x}", &item).is_err());
        assert!(render("{category!lower}", &item).is_err());
    }

    #[test]
    fn string_conversion_preserves_string_fields() {
        let item = sample();
        assert_eq!(render("{filename!s}", &item).unwrap(), "original");
        assert_eq!(render("{id!s}", &item).unwrap(), "42");
    }

    #[test]
    fn source_metadata_fields_and_string_lists_are_available_to_templates() {
        let item = sample();
        let metadata = serde_json::from_value::<Metadata>(serde_json::json!({
            "title": "Autumn Night",
            "tags": ["autumn", "night sky"],
            "count": 12,
            "private": null
        }))
        .unwrap();
        assert_eq!(
            render_with_metadata("{title!u}_{tags!S}_{count}", &item, &metadata).unwrap(),
            "AUTUMN NIGHT_autumn, night sky_12"
        );
        assert_eq!(
            render_with_metadata("{private|title}", &item, &metadata).unwrap(),
            "Autumn Night"
        );
        assert_eq!(
            render_with_metadata("{missing|title}", &item, &metadata).unwrap(),
            "Autumn Night"
        );
        assert!(render_with_metadata("{tags}", &item, &metadata).is_err());
        assert!(render_with_metadata("{author}", &item, &metadata).is_err());
    }

    #[test]
    fn list_and_string_formatter_specs_compose() {
        let item = sample();
        let metadata: Metadata = serde_json::from_value(serde_json::json!({
            "tags": ["zeta", "alpha", "middle"],
            "title": "plain",
            "display": "  Mixed Case  ",
            "display_encoded": "Mixed%20Case",
            "display_ws": "one\t  TWO\nthree",
            "enabled": false,
            "count": 0,
            "positive": 2,
            "artists": [{"name": "one"}, {"name": "two"}, {"name": "three"}]
        }))
        .unwrap();
        assert_eq!(
            render_with_metadata("{tags:J, /}", &item, &metadata).unwrap(),
            "zeta, alpha, middle"
        );
        assert_eq!(
            render_with_metadata("{tags:J/}", &item, &metadata).unwrap(),
            "zetaalphamiddle"
        );
        assert_eq!(
            render_with_metadata("{tags[0]:J, /}", &item, &metadata).unwrap(),
            "zeta"
        );
        assert_eq!(
            render_with_metadata("{tags[-1]:J, /}", &item, &metadata).unwrap(),
            "middle"
        );
        assert_eq!(
            render_with_metadata("{tags[0:2]:J, /}", &item, &metadata).unwrap(),
            "zeta, alpha"
        );
        assert_eq!(
            render_with_metadata("{tags[::-1]:J, /}", &item, &metadata).unwrap(),
            "middle, alpha, zeta"
        );
        assert_eq!(
            render_with_metadata("{artists:Mname/J + /}", &item, &metadata).unwrap(),
            "one + two + three"
        );
        assert_eq!(
            render_with_metadata("{tags:S/J, /}", &item, &metadata).unwrap(),
            "alpha, middle, zeta"
        );
        assert_eq!(
            render_with_metadata("{tags:Sr/J, /}", &item, &metadata).unwrap(),
            "zeta, middle, alpha"
        );
        assert_eq!(
            render_with_metadata("{title:A*3/}", &item, &metadata).unwrap(),
            "plainplainplain"
        );
        assert_eq!(
            render_with_metadata("{display:Ctu/}", &item, &metadata).unwrap(),
            "MIXED CASE"
        );
        assert_eq!(
            render_with_metadata("{display:Cg/}", &item, &metadata).unwrap(),
            "mixed-case"
        );
        assert_eq!(
            render_with_metadata("{display:CW/}", &item, &metadata).unwrap(),
            "Mixed Case"
        );
        assert_eq!(
            render_with_metadata("{display_ws:CC/}", &item, &metadata).unwrap(),
            "One Two Three"
        );
        assert_eq!(
            render_with_metadata("{display_ws!C}", &item, &metadata).unwrap(),
            "One Two Three"
        );
        assert_eq!(
            convert_string_chain("  Mixed Case  ", "j").unwrap(),
            "\"  Mixed Case  \""
        );
        assert_eq!(
            convert_string_chain("  Mixed Case  ", "r").unwrap(),
            "'  Mixed Case  '"
        );
        assert_eq!(
            convert_string_chain("  Mixed Case  ", "a").unwrap(),
            "'  Mixed Case  '"
        );
        assert_eq!(
            render_with_metadata("{display:Cq/}", &item, &metadata).unwrap(),
            "%20%20Mixed%20Case%20%20"
        );
        assert_eq!(
            convert_string_chain("Mixed%20Case", "Q").unwrap(),
            "Mixed Case"
        );
        assert!(render_with_metadata("{title:A*-2/}", &item, &metadata).is_err());
        assert_eq!(
            render_with_metadata("{title:J, /}", &item, &metadata).unwrap(),
            "plain"
        );
        assert_eq!(
            render_with_metadata("{title:I}", &item, &metadata).unwrap(),
            "plain"
        );
        assert_eq!(
            render_with_metadata("{title:Iignored}", &item, &metadata).unwrap(),
            "plain"
        );
        assert_eq!(
            render_with_metadata("{title!u:I}", &item, &metadata).unwrap(),
            "PLAIN"
        );
        assert_eq!(
            render_with_metadata("{id:I}", &item, &metadata).unwrap(),
            "42"
        );
        assert_eq!(
            render_with_metadata("file{enabled:?yes/no/}", &item, &metadata).unwrap(),
            "file"
        );
        assert_eq!(
            render_with_metadata("file{enabled!s:?yes/no/}", &item, &metadata).unwrap(),
            "fileyesFalseno"
        );
        assert_eq!(
            render_with_metadata("file{count:?n-/times/}", &item, &metadata).unwrap(),
            "file"
        );
        assert_eq!(
            render_with_metadata("file{count!s:?n-/times/}", &item, &metadata).unwrap(),
            "filen-0times"
        );
        assert_eq!(
            render_with_metadata("file{positive:?n-/times/}", &item, &metadata).unwrap(),
            "filen-2times"
        );
        assert!(render_with_metadata("{id:J, /}", &item, &metadata).is_err());
        assert!(render_with_metadata("{tags[3]:J, /}", &item, &metadata).is_err());
    }

    #[test]
    fn named_fields_support_unicode_indices_and_slices() {
        let mut item = sample();
        item.filename = "猫original.webp".into();
        assert_eq!(render("{filename[0]}", &item).unwrap(), "猫");
        item.filename = "猫é.webp".into();
        assert_eq!(render("{filename:_^6}", &item).unwrap(), "__猫é__");
        assert_eq!(render("{filename:.1}", &item).unwrap(), "猫");
        item.filename = "猫original.webp".into();
        assert_eq!(render("{filename[-4:]}", &item).unwrap(), "inal");
        assert_eq!(render("{filename[1:4]!u}", &item).unwrap(), "ORI");
        assert_eq!(render("{filename[1:4:2]}", &item).unwrap(), "oi");
        assert_eq!(render("{filename[::-1]}", &item).unwrap(), "lanigiro猫");
        assert_eq!(render("{filename[4:1:-1]}", &item).unwrap(), "gir");
        assert_eq!(render("x{filename[99:]}", &item).unwrap(), "x");
        assert!(render("{filename[99]}", &item).is_err());
        assert!(render("{filename[::0]}", &item).is_err());
    }

    #[test]
    fn byte_slices_match_utf8_byte_offsets_and_ignore_invalid_boundaries() {
        let mut item = sample();
        item.filename = "asset.jpg".into();
        assert_eq!(render("{filename[b0:3]}", &item).unwrap(), "ass");
        assert_eq!(render("{filename[b1:5:2]}", &item).unwrap(), "se");
        assert_eq!(render("{filename[b::-1]}", &item).unwrap(), "tessa");
        item.filename = "猫x.jpg".into();
        assert_eq!(render("{stem[b0:3]}", &item).unwrap(), "猫");
        assert_eq!(slice_field_bytes("猫x", "1:4").unwrap(), "x");
        assert_eq!(render("{stem[b1:4]}_suffix", &item).unwrap(), "x_suffix");
        assert!(render("{stem[b0:2:0]}", &item).is_err());
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
    fn repr_and_ascii_conversions_quote_and_escape_strings() {
        let mut item = sample();
        item.filename = "a'b.txt".into();
        assert_eq!(field("filename!r", &item).unwrap(), "\"a'b\"");
        assert_eq!(field("id!r", &item).unwrap(), "42");
        assert_eq!(field("id!a", &item).unwrap(), "42");
        assert_eq!(field("_lit[a'b]!a", &item).unwrap(), "\"a'b\"");
        assert_eq!(field("_lit[a\"b']!r", &item).unwrap(), "'a\"b\\''");
        assert_eq!(
            field("_lit[line\nbreak]!r", &item).unwrap(),
            "'line\\nbreak'"
        );
        assert_eq!(field("_lit[é猫]!r", &item).unwrap(), "'é猫'");
        assert_eq!(field("_lit[é猫]!a", &item).unwrap(), "'\\xe9\\u732b'");
    }

    #[test]
    fn json_conversion_matches_ascii_escaped_python_string_json() {
        assert_eq!(field("_lit[élan]!j", &sample()).unwrap(), r#""\u00e9lan""#);
        assert_eq!(
            field("_lit[猫😀]!j", &sample()).unwrap(),
            r#""\u732b\ud83d\ude00""#
        );
        assert_eq!(
            field("_lit[line\nbreak]!j", &sample()).unwrap(),
            r#""line\nbreak""#
        );
        assert_eq!(
            field("_lit[quote\"slash\\]!j", &sample()).unwrap(),
            r#""quote\"slash\\""#
        );
    }

    #[test]
    fn slug_conversion_normalizes_basic_ascii_values_and_rejects_unicode() {
        let item = sample();
        assert_eq!(field("_lit[Hello, World]!g", &item).unwrap(), "hello-world");
        assert_eq!(
            field("_lit[Assets.Example.org]!g", &item).unwrap(),
            "assetsexampleorg"
        );
        assert_eq!(
            field("_lit[Assets.Example.org:8443]!g", &item).unwrap(),
            "assetsexampleorg8443"
        );
        assert_eq!(
            field("_lit[  Foo__BAR--baz  ]!g", &item).unwrap(),
            "foo-bar-baz"
        );
        assert!(field("_lit[猫]!g", &item).is_err());
    }

    #[test]
    fn alternative_fields_fall_back_from_empty_or_missing_values() {
        let mut item = sample();
        item.filename = "README".into();
        item.post_id = None;
        assert_eq!(render("{extension|filename}", &item).unwrap(), "README");
        assert_eq!(render("{post_id|category}", &item).unwrap(), "danbooru");
        assert!(render("{post_id}", &item).is_err());
        assert!(render("{filename||category}", &item).is_err());
        assert!(render("{filename|unsupported}", &item).is_err());
    }

    #[test]
    fn integer_arithmetic_specs_use_checked_operations() {
        let item = sample();
        assert_eq!(render("{id:A+8/}", &item).unwrap(), "50");
        assert_eq!(render("{id:A-2/}", &item).unwrap(), "40");
        assert_eq!(render("{id:A*3/}", &item).unwrap(), "126");
        assert!(render("{filename:A+1/}", &item).is_err());
        assert!(render("{id:A/}", &item).is_err());
        assert!(render("{id:A/2/}", &item).is_err());
        let mut large = item;
        large.filename = i128::MAX.to_string();
        assert!(render("{filename:A*4/}", &large).is_err());
    }

    #[test]
    fn integer_radix_specs_match_python_style_bases_and_padding() {
        let item = sample();
        assert_eq!(render("{id:d}", &item).unwrap(), "42");
        assert_eq!(render("{id:05d}", &item).unwrap(), "00042");
        assert_eq!(render("{id:b}", &item).unwrap(), "101010");
        assert_eq!(render("{id:#b}", &item).unwrap(), "0b101010");
        assert_eq!(render("{id:o}", &item).unwrap(), "52");
        assert_eq!(render("{id:#o}", &item).unwrap(), "0o52");
        assert_eq!(render("{id:x}", &item).unwrap(), "2a");
        assert_eq!(render("{id:8x}", &item).unwrap(), "      2a");
        assert_eq!(render("{id:08x}", &item).unwrap(), "0000002a");
        assert_eq!(render("{id:#08x}", &item).unwrap(), "0x00002a");
        assert!(render("{id:#065x}", &item).is_err());
        assert!(render("{id:X}", &item).is_err());
        assert!(render("{filename:x}", &item).is_err());
    }

    #[test]
    fn integer_decimal_specs_support_grouping_and_space_width() {
        let mut item = sample();
        item.post_id = Some(1_234_567);
        assert_eq!(render("{id:,}", &item).unwrap(), "1,234,567");
        assert_eq!(render("{id:_}", &item).unwrap(), "1_234_567");
        assert_eq!(render("{id:12,}", &item).unwrap(), "   1,234,567");
        assert!(render("{id:#,}", &item).is_err());
        assert!(render("{id:012,}", &item).is_err());
    }

    #[test]
    fn replacement_formatter_replaces_all_literal_occurrences() {
        let mut item = sample();
        item.filename = "red blue red.webp".into();
        assert_eq!(
            render("{stem:Rred/green/}", &item).unwrap(),
            "green blue green"
        );
        assert_eq!(
            render("{stem:Rorange/cyan/}", &item).unwrap(),
            "red blue red"
        );
        item.extractor = "directlink";
        assert_eq!(
            render("{category:Ri/I/Rl/L/}", &item).unwrap(),
            "dIrectLInk"
        );
        assert_eq!(
            render("{category:Ri/I/?[/]/}", &item).unwrap(),
            "[dIrectlInk]"
        );
        assert_eq!(render("{category:Ri/I/X5/-/}", &item).unwrap(), "dIre-");
        assert_eq!(render("{stem:Rred/-/}", &item).unwrap(), "- blue -");
        assert!(render("{stem:Rred/green}", &item).is_err());
        item.filename = "café-café.webp".into();
        assert_eq!(render("{stem:Rfé/猫/}", &item).unwrap(), "ca猫-ca猫");
        item.filename = "x".repeat(241);
        assert!(render("{stem:R/x/}", &item).is_err());
    }

    #[test]
    fn capitalization_uppercases_first_character_and_lowers_the_rest() {
        let mut item = sample();
        item.filename = "élÉPHant.webp".into();
        assert_eq!(field("filename!c", &item).unwrap(), "Éléphant");
        item.filename.clear();
        assert_eq!(field("filename!c", &item).unwrap(), "");
    }

    #[test]
    fn title_conversion_capitalizes_each_alphanumeric_word() {
        let mut item = sample();
        item.filename = "aUTUMN, ÉLAN vITAL.webp".into();
        assert_eq!(field("filename!C", &item).unwrap(), "Autumn, Élan Vital");
    }

    #[test]
    fn length_conversion_is_rejected_for_filename_output() {
        let item = sample();
        assert!(render("{_lit[é猫]!n}", &item).is_err());
    }

    #[test]
    fn trim_conversion_removes_outer_unicode_whitespace() {
        let mut item = sample();
        item.filename = "\u{2003}original.webp\t".into();
        assert_eq!(field("filename!t", &item).unwrap(), "original");
    }

    #[test]
    fn whitespace_conversion_collapses_runs_to_one_space() {
        let mut item = sample();
        item.filename = "  spring\t\n  rain.webp  ".into();
        assert_eq!(field("filename!W", &item).unwrap(), "spring rain");
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
