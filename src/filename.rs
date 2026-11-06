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
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn decode_url_component(value: &str) -> String {
    let decoded = percent_encoding::percent_decode_str(value).collect::<Vec<_>>();
    String::from_utf8_lossy(&decoded).into_owned()
}

fn language_name(code: &str) -> Option<&'static str> {
    match code.to_ascii_lowercase().as_str() {
        "ar" => Some("Arabic"),
        "bg" => Some("Bulgarian"),
        "bn" => Some("Bengali"),
        "ca" => Some("Catalan"),
        "cs" => Some("Czech"),
        "da" => Some("Danish"),
        "de" => Some("German"),
        "el" => Some("Greek"),
        "en" => Some("English"),
        "es" => Some("Spanish"),
        "fa" => Some("Persian"),
        "fi" => Some("Finnish"),
        "fr" => Some("French"),
        "he" => Some("Hebrew"),
        "hi" => Some("Hindi"),
        "hu" => Some("Hungarian"),
        "id" => Some("Indonesian"),
        "it" => Some("Italian"),
        "ja" => Some("Japanese"),
        "ko" => Some("Korean"),
        "ms" => Some("Malay"),
        "nl" => Some("Dutch"),
        "no" => Some("Norwegian"),
        "pl" => Some("Polish"),
        "pt" => Some("Portuguese"),
        "ro" => Some("Romanian"),
        "ru" => Some("Russian"),
        "sk" => Some("Slovak"),
        "sl" => Some("Slovenian"),
        "sr" => Some("Serbian"),
        "sv" => Some("Swedish"),
        "th" => Some("Thai"),
        "tr" => Some("Turkish"),
        "uk" => Some("Ukrainian"),
        "vi" => Some("Vietnamese"),
        "zh" => Some("Chinese"),
        _ => None,
    }
}

fn decode_html_entities(value: &str) -> String {
    const LEGACY_NAMES: &str = r"
        AElig AMP Aacute Acirc Agrave Aring Atilde Auml COPY Ccedil ETH Eacute Ecirc Egrave Euml
        GT Iacute Icirc Igrave Iuml LT Ntilde Oacute Ocirc Ograve Oslash Otilde Ouml QUOT REG
        THORN Uacute Ucirc Ugrave Uuml Yacute aacute acirc acute aelig agrave amp aring atilde auml
        brvbar ccedil cedil cent copy curren deg divide eacute ecirc egrave eth euml frac12 frac14
        frac34 gt iacute icirc iexcl igrave iquest iuml laquo lt macr micro middot nbsp not ntilde
        oacute ocirc ograve ordf ordm oslash otilde ouml para plusmn pound quot raquo reg sect shy
        sup1 sup2 sup3 szlig thorn times uacute ucirc ugrave uml uuml yacute yen yuml
    ";
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < value.len() {
        if value.as_bytes()[cursor] == b'&' {
            if let Some((decoded, end)) = decode_numeric_html_reference(value, cursor) {
                output.push_str(&decoded);
                cursor = end;
                continue;
            }
            if let Some(relative_end) = value[cursor..].find(';') {
                let end = cursor + relative_end + 1;
                if end - cursor <= 64 {
                    let candidate = &value[cursor..end];
                    let decoded = html_escape::decode_html_entities(candidate);
                    if decoded.as_ref() != candidate {
                        output.push_str(&decoded);
                        cursor = end;
                        continue;
                    }
                }
            }
            let suffix = &value[cursor + 1..];
            if let Some(name) = LEGACY_NAMES
                .split_ascii_whitespace()
                .filter(|name| suffix.starts_with(*name))
                .max_by_key(|name| name.len())
            {
                let entity = format!("&{name};");
                output.push_str(&html_escape::decode_html_entities(&entity));
                cursor += name.len() + 1;
                continue;
            }
        }
        let character = value[cursor..].chars().next().expect("cursor is in bounds");
        output.push(character);
        cursor += character.len_utf8();
    }
    output
}

fn decode_numeric_html_reference(value: &str, cursor: usize) -> Option<(String, usize)> {
    let bytes = value.as_bytes();
    if bytes.get(cursor..cursor + 2)? != b"&#" {
        return None;
    }
    let mut digits = cursor + 2;
    let radix = if bytes
        .get(digits)
        .is_some_and(|byte| matches!(*byte, b'x' | b'X'))
    {
        digits += 1;
        16u32
    } else {
        10u32
    };
    let first_digit = digits;
    let mut codepoint = 0u32;
    let mut overflow = false;
    while let Some(&byte) = bytes.get(digits) {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' if radix == 16 => u32::from(byte - b'a' + 10),
            b'A'..=b'F' if radix == 16 => u32::from(byte - b'A' + 10),
            _ => break,
        };
        overflow |= codepoint
            .checked_mul(radix)
            .and_then(|number| number.checked_add(digit))
            .is_none();
        codepoint = codepoint.saturating_mul(radix).saturating_add(digit);
        digits += 1;
    }
    if digits == first_digit {
        return None;
    }
    if bytes.get(digits) == Some(&b';') {
        digits += 1;
    }
    let codepoint = if overflow || codepoint == 0 || codepoint > 0x10_FFFF {
        0xFFFD
    } else {
        match codepoint {
            0x80 => 0x20AC,
            0x82 => 0x201A,
            0x83 => 0x0192,
            0x84 => 0x201E,
            0x85 => 0x2026,
            0x86 => 0x2020,
            0x87 => 0x2021,
            0x88 => 0x02C6,
            0x89 => 0x2030,
            0x8A => 0x0160,
            0x8B => 0x2039,
            0x8C => 0x0152,
            0x8E => 0x017D,
            0x91 => 0x2018,
            0x92 => 0x2019,
            0x93 => 0x201C,
            0x94 => 0x201D,
            0x95 => 0x2022,
            0x96 => 0x2013,
            0x97 => 0x2014,
            0x98 => 0x02DC,
            0x99 => 0x2122,
            0x9A => 0x0161,
            0x9B => 0x203A,
            0x9C => 0x0153,
            0x9E => 0x017E,
            0x9F => 0x0178,
            value => value,
        }
    };
    let text = if matches!(
        codepoint,
        0x01..=0x08 | 0x0B | 0x0E..=0x1F | 0x7F
    ) || (0xFDD0..=0xFDEF).contains(&codepoint)
        || codepoint & 0xFFFE == 0xFFFE
    {
        String::new()
    } else {
        char::from_u32(codepoint).unwrap_or('\u{FFFD}').to_string()
    };
    Some((text, digits))
}

fn html_to_text(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'<' {
            if value[cursor..].starts_with("<!--") {
                if let Some(end) = value[cursor + 4..].find("-->") {
                    output.push(' ');
                    cursor += 4 + end + 3;
                    continue;
                }
            }
            let mut end = cursor + 1;
            let mut quote = None;
            while end < bytes.len() {
                match (quote, bytes[end]) {
                    (Some(delimiter), byte) if delimiter == byte => quote = None,
                    (None, b'\'' | b'"') => quote = Some(bytes[end]),
                    (None, b'>') if end > cursor + 1 => break,
                    _ => {}
                }
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b'>' && end > cursor + 1 {
                output.push(' ');
                cursor = end + 1;
                continue;
            }
        }
        let character = value[cursor..].chars().next().expect("cursor is in bounds");
        output.push(character);
        cursor += character.len_utf8();
    }
    let collapsed = output.split_whitespace().collect::<Vec<_>>().join(" ");
    decode_html_entities(&collapsed)
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

#[derive(Clone, Copy)]
enum FieldSelector<'a> {
    Attribute(&'a str),
    Element(&'a str),
}

fn split_field_selectors(candidate: &str) -> Result<(&str, Vec<FieldSelector<'_>>), FormatError> {
    let Some(mut cursor) = candidate.find(['[', '.']) else {
        if candidate.contains(']') {
            return Err(FormatError("invalid filename field selector".into()));
        }
        return Ok((candidate, Vec::new()));
    };

    let base = &candidate[..cursor];
    let mut selectors = Vec::new();
    if base == "_lit" {
        let literal = candidate[cursor + 1..]
            .strip_suffix(']')
            .ok_or_else(|| FormatError("invalid filename field selector".into()))?;
        selectors.push(FieldSelector::Element(literal));
        return Ok((base, selectors));
    }
    while cursor < candidate.len() {
        if candidate.as_bytes().get(cursor) == Some(&b'.') {
            let start = cursor + 1;
            let end = candidate[start..]
                .find(['[', '.'])
                .map_or(candidate.len(), |offset| start + offset);
            if start == end {
                return Err(FormatError("invalid filename attribute selector".into()));
            }
            selectors.push(FieldSelector::Attribute(&candidate[start..end]));
            cursor = end;
        } else {
            if candidate.as_bytes().get(cursor) != Some(&b'[') {
                return Err(FormatError("invalid filename field selector".into()));
            }
            let start = cursor + 1;
            let mut quote = None;
            let mut escaped = false;
            let mut closing = None;
            for (offset, character) in candidate[start..].char_indices() {
                if escaped {
                    escaped = false;
                    continue;
                }
                if character == '\\' && quote.is_some() {
                    escaped = true;
                    continue;
                }
                if let Some(delimiter) = quote {
                    if character == delimiter {
                        quote = None;
                    }
                    continue;
                }
                match character {
                    '\'' | '"' => quote = Some(character),
                    ']' => {
                        closing = Some(start + offset);
                        break;
                    }
                    '[' => return Err(FormatError("invalid filename field selector".into())),
                    _ => {}
                }
            }
            let closing =
                closing.ok_or_else(|| FormatError("invalid filename field selector".into()))?;
            selectors.push(FieldSelector::Element(&candidate[start..closing]));
            cursor = closing + 1;
        }
    }
    Ok((base, selectors))
}

fn select_mapping_key(selector: &str) -> &str {
    selector
        .strip_prefix('\'')
        .and_then(|key| key.strip_suffix('\''))
        .or_else(|| {
            selector
                .strip_prefix('"')
                .and_then(|key| key.strip_suffix('"'))
        })
        .unwrap_or(selector)
}

fn select_value(
    value: &Value,
    selector: FieldSelector<'_>,
    field: &str,
) -> Result<Value, FormatError> {
    match (value, selector) {
        (Value::Array(values), FieldSelector::Element(selector)) => select_array(values, selector),
        (Value::Object(fields), FieldSelector::Element(selector)) => fields
            .get(select_mapping_key(selector))
            .cloned()
            .ok_or_else(|| {
                FormatError(format!(
                    "filename mapping field '{field}[{selector}]' is missing"
                ))
            }),
        (Value::String(value), FieldSelector::Element(selector)) => {
            slice_field(value, selector).map(Value::String)
        }
        (Value::Object(fields), FieldSelector::Attribute(selector)) => {
            fields.get(selector).cloned().ok_or_else(|| {
                FormatError(format!(
                    "filename attribute field '{field}.{selector}' is missing"
                ))
            })
        }
        (_, FieldSelector::Attribute(selector)) => Err(FormatError(format!(
            "filename field '{field}' has no attribute '{selector}'"
        ))),
        _ => Err(FormatError(format!(
            "filename field '{field}' does not support element selection"
        ))),
    }
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

fn python_json_value(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => python_json_string(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_json_value)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| {
                    format!("{}:{}", python_json_string(key), python_json_value(value))
                })
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn slugify(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_whitespace() || character == '-' {
            separator = true;
        } else if character.is_alphanumeric() || character == '_' {
            if separator && !result.is_empty() {
                result.push('-');
            }
            result.push(character);
            separator = false;
        }
    }
    result.trim_matches(['-', '_']).to_owned()
}

fn format_scientific_float(number: f64, precision: usize, uppercase: bool) -> String {
    let raw = format!("{number:.precision$e}");
    let (mantissa, exponent) = raw
        .split_once('e')
        .expect("Rust scientific formatting includes an exponent");
    let exponent = exponent
        .parse::<i32>()
        .expect("Rust scientific exponent is a signed integer");
    let exponent_sign = if exponent < 0 { '-' } else { '+' };
    let marker = if uppercase { 'E' } else { 'e' };
    format!(
        "{mantissa}{marker}{exponent_sign}{:02}",
        exponent.unsigned_abs()
    )
}

fn format_general_float(number: f64, precision: usize, uppercase: bool, alternate: bool) -> String {
    let precision = precision.max(1);
    if number.is_nan() {
        return if uppercase { "NAN" } else { "nan" }.to_owned();
    }
    if number.is_infinite() {
        let sign = if number.is_sign_negative() { "-" } else { "" };
        let value = if uppercase { "INF" } else { "inf" };
        return format!("{sign}{value}");
    }
    if number == 0.0 {
        let sign = if number.is_sign_negative() { "-" } else { "" };
        return if alternate {
            format!("{sign}0.{}", "0".repeat(precision - 1))
        } else {
            format!("{sign}0")
        };
    }
    let scientific = format!("{number:.digits$e}", digits = precision - 1);
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("Rust scientific formatting includes an exponent");
    let exponent = exponent
        .parse::<i32>()
        .expect("Rust scientific exponent is a signed integer");
    if exponent < -4 || exponent >= precision as i32 {
        let mantissa = if alternate {
            if mantissa.contains('.') {
                mantissa.to_owned()
            } else {
                format!("{mantissa}.")
            }
        } else {
            mantissa
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_owned()
        };
        let marker = if uppercase { 'E' } else { 'e' };
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}{marker}{sign}{:02}", exponent.unsigned_abs());
    }
    let decimals = (precision as i32 - exponent - 1).max(0) as usize;
    let fixed = format!("{number:.decimals$}");
    if fixed.contains('.') && !alternate {
        fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        fixed
    }
}

fn format_fixed_numeric(value: &Value, spec: &str) -> Result<Option<String>, FormatError> {
    let Some(kind @ ('f' | 'F' | 'e' | 'E' | 'g' | 'G' | 'n' | '%')) = spec.chars().last() else {
        return Ok(None);
    };
    if kind == 'n'
        && value
            .as_number()
            .is_some_and(|number| number.as_i64().is_some() || number.as_u64().is_some())
    {
        // Keep integer `n` formatting on the exact decimal path; converting
        // large integers to f64 would lose precision.
        return Ok(None);
    }
    let percent = kind == '%';
    let options = &spec[..spec.len() - kind.len_utf8()];
    let (raw_width, precision) = match options.split_once('.') {
        Some((width, precision)) if precision.bytes().all(|byte| byte.is_ascii_digit()) => {
            let precision = precision
                .parse::<usize>()
                .map_err(|_| FormatError("invalid numeric precision".into()))?;
            (width, precision)
        }
        Some(_) => return Err(FormatError("invalid numeric precision".into())),
        None => (options, 6),
    };
    let mut width_chars = raw_width.char_indices();
    let first = width_chars.next();
    let second = width_chars.next();
    let (alignment, mut fill, explicit_fill, width_spec) = match (first, second) {
        (Some((_, fill)), Some((end, alignment @ ('<' | '>' | '^' | '='))))
            if !matches!(fill, '{' | '}') =>
        {
            (
                Some(alignment),
                fill.to_string(),
                true,
                &raw_width[end + alignment.len_utf8()..],
            )
        }
        (Some((0, alignment @ ('<' | '>' | '^' | '='))), _) => (
            Some(alignment),
            " ".to_owned(),
            false,
            &raw_width[alignment.len_utf8()..],
        ),
        _ => (None, " ".to_owned(), false, raw_width),
    };
    let (sign_option, width_spec) = match width_spec.chars().next() {
        Some(sign @ ('+' | '-' | ' ')) => (Some(sign), &width_spec[sign.len_utf8()..]),
        _ => (None, width_spec),
    };
    let alternate = width_spec.starts_with('#');
    let width_spec = if alternate {
        &width_spec[1..]
    } else {
        width_spec
    };
    let zero_flag = width_spec.starts_with('0') && width_spec.len() > 1;
    let zero_fill = alignment.is_none() && zero_flag;
    if zero_flag && !explicit_fill {
        fill = "0".to_owned();
    }
    let width_text = if zero_fill {
        &width_spec[1..]
    } else {
        width_spec
    };
    let (width_digits, grouping) = match width_text.find([',', '_']) {
        Some(index) if width_text[index + 1..].is_empty() => {
            (&width_text[..index], width_text[index..].chars().next())
        }
        Some(_) => {
            return Err(FormatError(
                "unsupported numeric format specification".into(),
            ));
        }
        None => (width_text, None),
    };
    if kind == 'n' && grouping.is_some() {
        return Err(FormatError(
            "locale numeric formatting does not accept explicit grouping".into(),
        ));
    }
    if !width_digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FormatError(
            "unsupported numeric format specification".into(),
        ));
    }
    let width = if width_digits.is_empty() {
        0
    } else {
        width_digits
            .parse::<usize>()
            .map_err(|_| FormatError("invalid numeric width".into()))?
    };
    if width > 240 || precision > 200 {
        return Err(FormatError("numeric format exceeds filename limits".into()));
    }
    let Some(number) = value.as_number() else {
        return Ok(None);
    };
    let sign = match (
        number.as_i64().is_some_and(|number| number < 0)
            || number
                .as_f64()
                .is_some_and(|number| number.is_sign_negative()),
        sign_option,
    ) {
        (true, _) => "-",
        (false, Some('+')) => "+",
        (false, Some(' ')) => " ",
        _ => "",
    };
    let decimal_zeros = if precision == 0 {
        String::new()
    } else {
        format!(".{}", "0".repeat(precision))
    };
    let mut output = if percent {
        let suffix = "%";
        if let Some(integer) = number.as_i64() {
            let scaled = integer
                .checked_mul(100)
                .ok_or_else(|| FormatError("percentage formatting overflow".into()))?;
            format!("{scaled}{decimal_zeros}{suffix}")
        } else if let Some(integer) = number.as_u64() {
            let scaled = integer
                .checked_mul(100)
                .ok_or_else(|| FormatError("percentage formatting overflow".into()))?;
            format!("{scaled}{decimal_zeros}{suffix}")
        } else {
            let number = number
                .as_f64()
                .ok_or_else(|| FormatError("invalid numeric value".into()))?
                * 100.0;
            format!("{number:.precision$}%")
        }
    } else if matches!(kind, 'e' | 'E') {
        let number = number
            .as_f64()
            .ok_or_else(|| FormatError("invalid numeric value".into()))?;
        format_scientific_float(number, precision, kind == 'E')
    } else if matches!(kind, 'g' | 'G' | 'n') {
        let number = number
            .as_f64()
            .ok_or_else(|| FormatError("invalid numeric value".into()))?;
        format_general_float(number, precision, kind == 'G', alternate)
    } else if let Some(integer) = number.as_i64() {
        format!("{integer}{decimal_zeros}")
    } else if let Some(integer) = number.as_u64() {
        format!("{integer}{decimal_zeros}")
    } else {
        let number = number
            .as_f64()
            .ok_or_else(|| FormatError("invalid numeric value".into()))?;
        format!("{number:.precision$}")
    };
    if alternate && !output.contains('.') {
        let insertion = output.find(['e', 'E', '%']).unwrap_or(output.len());
        output.insert(insertion, '.');
    }
    if let Some(separator) = grouping {
        if zero_fill {
            let mut candidate = output.clone();
            loop {
                let grouped = group_fixed_float(&candidate, separator);
                let sign_width = usize::from(!sign.is_empty() && !candidate.starts_with('-'));
                if grouped.len() + sign_width >= width {
                    output = grouped;
                    break;
                }
                let integer_start = usize::from(candidate.starts_with('-'));
                candidate.insert(integer_start, '0');
            }
        } else {
            output = group_fixed_float(&output, separator);
        }
    }
    if !sign.is_empty() && !output.starts_with('-') {
        output.insert_str(0, sign);
    }
    if output.len() < width {
        let padding = width - output.len();
        if zero_fill {
            let first = output.chars().next().unwrap_or_default();
            if matches!(first, '-' | '+' | ' ') {
                output.insert_str(first.len_utf8(), &"0".repeat(padding));
            } else {
                output.insert_str(0, &"0".repeat(padding));
            }
        } else if alignment == Some('=') {
            let first = output.chars().next().unwrap_or_default();
            if matches!(first, '-' | '+' | ' ') {
                let split = first.len_utf8();
                output.insert_str(split, &fill.repeat(padding));
            } else {
                output.insert_str(0, &fill.repeat(padding));
            }
        } else {
            let (left, right) = match alignment {
                Some('<') => (0, padding),
                Some('^') => (padding / 2, padding - padding / 2),
                _ => (padding, 0),
            };
            output.insert_str(0, &fill.repeat(left));
            output.push_str(&fill.repeat(right));
        }
    }
    Ok(Some(output))
}

fn group_integer_digits(digits: &str, separator: char, group_size: usize) -> String {
    let mut reversed = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().rev().enumerate() {
        if index > 0 && index % group_size == 0 {
            reversed.push(separator);
        }
        reversed.push(digit);
    }
    reversed.chars().rev().collect()
}

fn group_fixed_float(value: &str, separator: char) -> String {
    let suffix_start = value.find(['e', 'E', '%']).unwrap_or(value.len());
    let decimal_start = value[..suffix_start].find('.').unwrap_or(suffix_start);
    let integer_start = usize::from(value.starts_with('-'));
    let grouped = group_integer_digits(&value[integer_start..decimal_start], separator, 3);
    format!(
        "{}{grouped}{}",
        &value[..integer_start],
        &value[decimal_start..]
    )
}

fn format_integer_character(value: &str, spec: &str) -> Result<Option<String>, FormatError> {
    if !spec.ends_with('c') {
        return Ok(None);
    }
    let options = &spec[..spec.len() - 1];
    let mut spec_chars = options.char_indices();
    let first = spec_chars.next();
    let second = spec_chars.next();
    let (alignment, mut fill, explicit_fill, width_spec) = match (first, second) {
        (Some((_, fill)), Some((end, alignment @ ('<' | '>' | '^' | '='))))
            if !matches!(fill, '{' | '}') =>
        {
            (
                Some(alignment),
                fill.to_string(),
                true,
                &options[end + alignment.len_utf8()..],
            )
        }
        (Some((0, alignment @ ('<' | '>' | '^' | '='))), _) => (
            Some(alignment),
            " ".to_owned(),
            false,
            &options[alignment.len_utf8()..],
        ),
        _ => (None, " ".to_owned(), false, options),
    };
    let zero_fill = alignment.is_none() && width_spec.starts_with('0') && width_spec.len() > 1;
    if zero_fill && !explicit_fill {
        fill = "0".to_owned();
    }
    let width_text = if zero_fill {
        &width_spec[1..]
    } else {
        width_spec
    };
    if !width_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FormatError(
            "unsupported integer character format specification".into(),
        ));
    }
    let width = if width_text.is_empty() {
        0
    } else {
        width_text
            .parse::<usize>()
            .map_err(|_| FormatError("invalid numeric width".into()))?
    };
    if width > 240 {
        return Err(FormatError("filename numeric width is too large".into()));
    }
    let codepoint = value
        .parse::<u32>()
        .map_err(|_| FormatError("integer character format requires a valid code point".into()))?;
    let character = char::from_u32(codepoint).ok_or_else(|| {
        FormatError("integer character format requires a valid code point".into())
    })?;
    let mut output = character.to_string();
    let padding = width.saturating_sub(output.chars().count());
    if zero_fill {
        output.insert_str(0, &"0".repeat(padding));
    } else {
        let (left, right) = match alignment {
            Some('<') => (0, padding),
            Some('^') => (padding / 2, padding - padding / 2),
            _ => (padding, 0),
        };
        output.insert_str(0, &fill.repeat(left));
        output.push_str(&fill.repeat(right));
    }
    Ok(Some(output))
}

fn format_integer(value: &str, spec: &str) -> Result<Option<String>, FormatError> {
    let Some((radix, _suffix)) = spec.split_at_checked(spec.len().saturating_sub(1)) else {
        return Ok(None);
    };
    let Some(kind) = spec.chars().last() else {
        return Ok(None);
    };
    if kind == 'n' && normalize_python_integer_text(value).is_none() {
        return Ok(None);
    }
    let (base, marker, type_grouping) = match kind {
        // `n` uses the current locale in Python's numeric mini-language.
        // The pinned formatter process starts in the C numeric locale, where
        // its integer representation is the ordinary decimal form.
        'd' | 'n' => (10, "", None),
        'b' => (2, "0b", None),
        'o' => (8, "0o", None),
        'x' => (16, "0x", None),
        ',' => (10, "", Some(',')),
        '_' => (10, "", Some('_')),
        _ => return Ok(None),
    };
    // Integer subset of Python's numeric mini-language with bounded width,
    // optional alternate prefixes, and explicit text alignment.
    let mut spec_chars = radix.char_indices();
    let first = spec_chars.next();
    let second = spec_chars.next();
    let (alignment, mut fill, explicit_fill, radix) = match (first, second) {
        (Some((_, fill)), Some((end, alignment @ ('<' | '>' | '^' | '='))))
            if !matches!(fill, '{' | '}') =>
        {
            (
                Some(alignment),
                fill.to_string(),
                true,
                &radix[end + alignment.len_utf8()..],
            )
        }
        (Some((0, alignment @ ('<' | '>' | '^' | '='))), _) => (
            Some(alignment),
            " ".to_owned(),
            false,
            &radix[alignment.len_utf8()..],
        ),
        _ => (None, " ".to_owned(), false, radix),
    };
    let (sign_option, radix) = match radix.chars().next() {
        Some(sign @ ('+' | '-' | ' ')) => (Some(sign), &radix[sign.len_utf8()..]),
        _ => (None, radix),
    };
    let alternate = radix.starts_with('#');
    let radix = radix.strip_prefix('#').unwrap_or(radix);
    let (grouping, width_spec) = match type_grouping {
        Some(separator) => (Some(separator), radix),
        None if base != 10 && radix.ends_with('_') => (Some('_'), &radix[..radix.len() - 1]),
        None => (None, radix),
    };
    let group_size = if base == 10 { 3 } else { 4 };
    let zero_flag = width_spec.starts_with('0') && width_spec.len() > 1;
    let zero_fill = alignment.is_none() && zero_flag;
    if zero_flag && !explicit_fill {
        fill = "0".to_owned();
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
    let integer = normalize_python_integer_text(value)
        .ok_or_else(|| FormatError("numeric formatting requires an integer field".into()))?;
    let negative = integer.starts_with('-');
    let sign = if negative {
        "-"
    } else {
        match sign_option {
            Some('+') => "+",
            Some(' ') => " ",
            _ => "",
        }
    };
    let magnitude = integer.strip_prefix('-').unwrap_or(&integer);
    let mut digits = match base {
        10 => magnitude.to_owned(),
        2 => decimal_magnitude_to_radix(magnitude, 2),
        8 => decimal_magnitude_to_radix(magnitude, 8),
        16 => decimal_magnitude_to_radix(magnitude, 16),
        _ => unreachable!(),
    };
    let prefix = if alternate { marker } else { "" };
    if zero_fill {
        if let Some(separator) = grouping {
            while sign.len()
                + prefix.len()
                + group_integer_digits(&digits, separator, group_size).len()
                < width
            {
                digits.insert(0, '0');
            }
            digits = group_integer_digits(&digits, separator, group_size);
        } else {
            let padding = width.saturating_sub(sign.len() + prefix.len() + digits.len());
            digits.insert_str(0, &"0".repeat(padding));
        }
        Ok(Some(format!("{sign}{prefix}{digits}")))
    } else {
        if let Some(separator) = grouping {
            digits = group_integer_digits(&digits, separator, group_size);
        }
        let padding = width.saturating_sub(sign.len() + prefix.len() + digits.len());
        if alignment == Some('=') {
            return Ok(Some(format!(
                "{sign}{prefix}{}{digits}",
                fill.repeat(padding)
            )));
        }
        let (left, right) = match alignment {
            Some('<') => (0, padding),
            Some('^') => (padding / 2, padding - padding / 2),
            _ => (padding, 0),
        };
        Ok(Some(format!(
            "{}{sign}{prefix}{digits}{}",
            fill.repeat(left),
            fill.repeat(right)
        )))
    }
}

fn decimal_magnitude_to_radix(value: &str, radix: u8) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    if value == "0" {
        return "0".to_owned();
    }
    let mut decimal = value.bytes().map(|byte| byte - b'0').collect::<Vec<_>>();
    let mut first = 0;
    let mut output = Vec::new();
    while first < decimal.len() {
        let mut remainder = 0_u16;
        for digit in &mut decimal[first..] {
            let current = remainder * 10 + u16::from(*digit);
            *digit = (current / u16::from(radix)) as u8;
            remainder = current % u16::from(radix);
        }
        output.push(DIGITS[usize::from(remainder)]);
        while first < decimal.len() && decimal[first] == 0 {
            first += 1;
        }
    }
    output.reverse();
    String::from_utf8(output).expect("numeric radix digits are ASCII")
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
            'S' => value,
            's' => value,
            'g' => slugify(&value),
            'j' => python_json_string(&value),
            'r' => python_string_repr(&value, false),
            'a' => python_string_repr(&value, true),
            'q' => encode_url_component(&value),
            'Q' => decode_url_component(&value),
            'U' => decode_html_entities(&value),
            'H' => html_to_text(&value),
            'L' => language_name(&value).map(str::to_owned).ok_or_else(|| {
                FormatError("language conversion produces a non-string filename value".into())
            })?,
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

fn extract_urls(value: &str) -> Vec<Value> {
    let mut urls = Vec::new();
    let mut search_from = 0;
    while search_from < value.len() {
        let next = ["http://", "https://"]
            .into_iter()
            .filter_map(|prefix| {
                value[search_from..]
                    .find(prefix)
                    .map(|offset| (offset, prefix))
            })
            .min_by_key(|(offset, _)| *offset);
        let Some((offset, prefix)) = next else {
            break;
        };
        let start = search_from + offset;
        let end = value[start..]
            .char_indices()
            .find_map(|(offset, character)| {
                (character.is_whitespace() || matches!(character, '"' | '\'' | '<' | '>' | '\\'))
                    .then_some(start + offset)
            })
            .unwrap_or(value.len());
        let end = end.max(start + prefix.len());
        urls.push(Value::String(value[start..end].to_owned()));
        search_from = end;
    }
    urls
}

fn python_array_repr(values: &[Value]) -> Result<String, FormatError> {
    let values = values
        .iter()
        .map(python_repr_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("[{}]", values.join(", ")))
}

fn python_array_ascii_repr(values: &[Value]) -> Result<String, FormatError> {
    let values = values
        .iter()
        .map(python_ascii_repr_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("[{}]", values.join(", ")))
}

fn python_ascii_repr_value(value: &Value) -> Result<String, FormatError> {
    match value {
        Value::String(value) => Ok(python_string_repr(value, true)),
        Value::Array(values) => python_array_ascii_repr(values),
        Value::Object(values) => {
            let values = values
                .iter()
                .map(|(key, value)| {
                    Ok(format!(
                        "{}: {}",
                        python_string_repr(key, true),
                        python_ascii_repr_value(value)?
                    ))
                })
                .collect::<Result<Vec<_>, FormatError>>()?;
            Ok(format!("{{{}}}", values.join(", ")))
        }
        _ => python_string_value(value),
    }
}

fn python_object_repr(values: &serde_json::Map<String, Value>) -> Result<String, FormatError> {
    let values = values
        .iter()
        .map(|(key, value)| {
            Ok(format!(
                "{}: {}",
                python_string_repr(key, false),
                python_repr_value(value)?
            ))
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    Ok(format!("{{{}}}", values.join(", ")))
}

fn python_repr_value(value: &Value) -> Result<String, FormatError> {
    match value {
        Value::String(value) => Ok(python_string_repr(value, false)),
        Value::Array(values) => python_array_repr(values),
        Value::Object(values) => python_object_repr(values),
        _ => python_string_value(value),
    }
}

fn python_string_value(value: &Value) -> Result<String, FormatError> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(true) => Ok("True".into()),
        Value::Bool(false) => Ok("False".into()),
        Value::Null => Ok("None".into()),
        Value::Array(values) => python_array_repr(values),
        Value::Object(values) => python_object_repr(values),
    }
}

fn formatter_to_string(value: &Value) -> Result<String, FormatError> {
    match value {
        Value::Null | Value::Bool(false) => Ok(String::new()),
        Value::Number(number)
            if number.as_i64() == Some(0)
                || number.as_u64() == Some(0)
                || number.as_f64() == Some(0.0) =>
        {
            Ok(String::new())
        }
        Value::String(value) => Ok(value.clone()),
        Value::Array(values) => values
            .iter()
            .map(python_string_value)
            .collect::<Result<Vec<_>, _>>()
            .map(|values| values.join(", ")),
        Value::Object(values) if values.is_empty() => Ok(String::new()),
        _ => python_string_value(value),
    }
}

fn python_integer_value(source: Option<&Value>, value: &str) -> Result<String, FormatError> {
    let invalid = || FormatError("integer conversion requires an integer-compatible value".into());
    match source {
        Some(Value::Bool(value)) => Ok(i128::from(*value).to_string()),
        Some(Value::Number(number)) => {
            if let Some(value) = number.as_i64() {
                Ok(value.to_string())
            } else if let Some(value) = number.as_u64() {
                Ok(value.to_string())
            } else if let Some(value) = number.as_f64() {
                python_integer_from_float(value).ok_or_else(invalid)
            } else {
                Err(invalid())
            }
        }
        Some(Value::String(value)) => normalize_python_integer_text(value).ok_or_else(invalid),
        Some(Value::Null | Value::Array(_) | Value::Object(_)) => Err(invalid()),
        None => normalize_python_integer_text(value).ok_or_else(invalid),
    }
}

fn python_integer_from_float(value: f64) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    let bits = value.to_bits();
    let negative = bits >> 63 != 0;
    let exponent_bits = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1_u64 << 52) - 1);
    let (significand, exponent) = if exponent_bits == 0 {
        (fraction, -1022 - 52)
    } else {
        ((1_u64 << 52) | fraction, exponent_bits - 1023 - 52)
    };
    let mut digits = if exponent >= 0 {
        let mut digits = significand.to_string().into_bytes();
        for _ in 0..exponent {
            let mut carry = 0_u8;
            for digit in digits.iter_mut().rev() {
                let doubled = (*digit - b'0') * 2 + carry;
                *digit = b'0' + doubled % 10;
                carry = doubled / 10;
            }
            if carry != 0 {
                digits.insert(0, b'0' + carry);
            }
        }
        String::from_utf8(digits).expect("decimal float integer is ASCII")
    } else {
        let right_shift = exponent.unsigned_abs();
        if right_shift >= u64::BITS {
            "0".to_owned()
        } else {
            (significand >> right_shift).to_string()
        }
    };
    if negative && digits != "0" {
        digits.insert(0, '-');
    }
    Some(digits)
}

fn normalize_python_integer_text(value: &str) -> Option<String> {
    // Match CPython's default decimal string-to-int digit guard.
    const MAX_DECIMAL_DIGITS: usize = 4_300;
    let value = value.trim();
    if value.len() > MAX_DECIMAL_DIGITS * 2 + 1 {
        return None;
    }
    let bytes = value.as_bytes();
    let digits_start = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    if digits_start == bytes.len() {
        return None;
    }
    let mut decimal_digits = 0;
    for (index, byte) in bytes.iter().enumerate().skip(digits_start) {
        if *byte == b'_' {
            if index == digits_start
                || !bytes[index - 1].is_ascii_digit()
                || !bytes.get(index + 1).is_some_and(u8::is_ascii_digit)
            {
                return None;
            }
        } else {
            if !byte.is_ascii_digit() {
                return None;
            }
            decimal_digits += 1;
            if decimal_digits > MAX_DECIMAL_DIGITS {
                return None;
            }
        }
    }
    let digits = value[digits_start..].replace('_', "");
    let digits = digits.trim_start_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    Some(if bytes.first() == Some(&b'-') && digits != "0" {
        format!("-{digits}")
    } else {
        digits.to_owned()
    })
}

fn python_float_value(source: Option<&Value>, value: &str) -> Result<f64, FormatError> {
    let invalid = || FormatError("float conversion requires a finite numeric value".into());
    let number = match source {
        Some(Value::Bool(value)) => f64::from(u8::from(*value)),
        Some(Value::Number(number)) => number.as_f64().ok_or_else(invalid)?,
        Some(Value::String(value)) => parse_python_float_text(value).ok_or_else(invalid)?,
        Some(Value::Null | Value::Array(_) | Value::Object(_)) => return Err(invalid()),
        None => parse_python_float_text(value).ok_or_else(invalid)?,
    };
    number.is_finite().then_some(number).ok_or_else(invalid)
}

fn parse_python_float_text(value: &str) -> Option<f64> {
    let value = value.trim();
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'_'
            && !(index > 0
                && bytes[index - 1].is_ascii_digit()
                && bytes.get(index + 1).is_some_and(u8::is_ascii_digit))
        {
            return None;
        }
    }
    value.replace('_', "").parse().ok()
}

fn python_length_value(source: Option<&Value>, value: &str) -> Result<usize, FormatError> {
    match source {
        Some(Value::String(value)) => Ok(value.chars().count()),
        Some(Value::Array(values)) => Ok(values.len()),
        Some(Value::Object(values)) => Ok(values.len()),
        None => Ok(value.chars().count()),
        Some(Value::Null | Value::Bool(_) | Value::Number(_)) => Err(FormatError(
            "length conversion requires a string, list, or mapping".into(),
        )),
    }
}

fn arithmetic_operation(spec: &str) -> Result<(&str, i128, &str), FormatError> {
    let operation = spec
        .strip_prefix('A')
        .ok_or_else(|| FormatError("invalid filename arithmetic formatter".into()))?;
    let (expression, remainder) = operation
        .split_once('/')
        .ok_or_else(|| FormatError("invalid filename arithmetic formatter".into()))?;
    let (operator, operand) = expression
        .split_at_checked(1)
        .ok_or_else(|| FormatError("invalid filename arithmetic formatter".into()))?;
    let right = operand
        .parse::<i128>()
        .map_err(|_| FormatError("invalid filename arithmetic operand".into()))?;
    Ok((operator, right, remainder))
}

fn apply_chained_replacements(
    mut value: String,
    mut spec: &str,
) -> Result<(String, &str), FormatError> {
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
    Ok((value, spec))
}

fn apply_arithmetic_string_conversion(value: String, spec: &str) -> Result<String, FormatError> {
    let (conversions, remainder) =
        conversion_formatter_spec(spec).expect("conversion formatter starts with C");
    let Some(conversions) = conversions.strip_prefix('S') else {
        return Err(FormatError(
            "filename non-string conversion requires the S conversion".into(),
        ));
    };
    let value = if conversions.is_empty() {
        value
    } else {
        convert_string_chain(&value, conversions)?
    };
    if remainder.is_empty() {
        Ok(value)
    } else {
        apply_string_formatter_chain(value, remainder)
    }
}

fn apply_integer_arithmetic_chain(left: i128, spec: &str) -> Result<String, FormatError> {
    let (operator, right, remainder) = arithmetic_operation(spec)?;
    let result = match operator {
        "+" => left.checked_add(right),
        "-" => left.checked_sub(right),
        "*" => left.checked_mul(right),
        // gallery-dl parses every arithmetic operator but only implements
        // addition, subtraction, and multiplication. Unknown operators leave
        // the value unchanged and continue through the remaining formatter.
        _ => Some(left),
    }
    .ok_or_else(|| FormatError("filename arithmetic overflow".into()))?;
    if remainder.starts_with('A') {
        return apply_integer_arithmetic_chain(result, remainder);
    }
    let value = result.to_string();
    if remainder.starts_with('R') {
        return Err(FormatError(
            "filename replacement requires a string value".into(),
        ));
    }
    if remainder.starts_with('C') {
        return apply_arithmetic_string_conversion(value, remainder);
    }
    if remainder.is_empty() {
        return Ok(value);
    }
    if let Some(formatted) = format_integer_character(&value, remainder)? {
        return Ok(formatted);
    }
    if let Some(formatted) = format_integer(&value, remainder)? {
        return Ok(formatted);
    }
    Err(FormatError(
        "unsupported filename arithmetic format suffix".into(),
    ))
}

fn apply_float_arithmetic_chain(left: f64, spec: &str) -> Result<String, FormatError> {
    let (operator, right, remainder) = arithmetic_operation(spec)?;
    let right = right as f64;
    let result = match operator {
        "+" => left + right,
        "-" => left - right,
        "*" => left * right,
        _ => left,
    };
    if !result.is_finite() {
        return Err(FormatError("filename arithmetic overflow".into()));
    }
    if remainder.starts_with('A') {
        return apply_float_arithmetic_chain(result, remainder);
    }
    let number = Value::Number(
        serde_json::Number::from_f64(result)
            .ok_or_else(|| FormatError("filename arithmetic overflow".into()))?,
    );
    if remainder.is_empty() {
        return formatter_to_string(&number);
    }
    if remainder.starts_with('R') {
        return Err(FormatError(
            "filename replacement requires a string value".into(),
        ));
    }
    if remainder.starts_with('C') {
        return apply_arithmetic_string_conversion(formatter_to_string(&number)?, remainder);
    }
    if let Some(formatted) = format_fixed_numeric(&number, remainder)? {
        return Ok(formatted);
    }
    Err(FormatError(
        "unsupported floating-point arithmetic format suffix".into(),
    ))
}

fn apply_string_arithmetic_chain(value: String, spec: &str) -> Result<String, FormatError> {
    let (operator, right, remainder) = arithmetic_operation(spec)?;
    if matches!(operator, "+" | "-") {
        return Err(FormatError(
            "filename arithmetic requires an integer field".into(),
        ));
    }
    let value = if operator == "*" {
        let count = usize::try_from(right.max(0))
            .map_err(|_| FormatError("filename repetition count is too large".into()))?;
        let output_len = value
            .len()
            .checked_mul(count)
            .filter(|length| *length <= 240)
            .ok_or_else(|| FormatError("filename repetition exceeds 240 bytes".into()))?;
        let repeated = value.repeat(count);
        debug_assert_eq!(repeated.len(), output_len);
        repeated
    } else {
        value
    };
    if remainder.starts_with('A') {
        return apply_string_arithmetic_chain(value, remainder);
    }
    apply_string_formatter_chain(value, remainder)
}

fn apply_array_arithmetic_chain(
    values: &[Value],
    field_name: &str,
    spec: &str,
) -> Result<String, FormatError> {
    let (operator, right, remainder) = arithmetic_operation(spec)?;
    if matches!(operator, "+" | "-") {
        return Err(FormatError(
            "filename arithmetic requires a numeric field".into(),
        ));
    }
    let repeated = if operator == "*" {
        let count = usize::try_from(right.max(0))
            .map_err(|_| FormatError("filename repetition count is too large".into()))?;
        let item_count = values
            .len()
            .checked_mul(count)
            .filter(|length| *length <= 240)
            .ok_or_else(|| FormatError("filename list repetition exceeds 240 items".into()))?;
        let mut repeated = Vec::with_capacity(item_count);
        if item_count != 0 {
            for _ in 0..count {
                repeated.extend(values.iter().cloned());
            }
        }
        debug_assert_eq!(repeated.len(), item_count);
        repeated
    } else {
        values.to_vec()
    };
    let result = if remainder.starts_with('A') {
        return apply_array_arithmetic_chain(&repeated, field_name, remainder);
    } else {
        let mut suffix = remainder;
        let result = metadata_field(&Value::Array(repeated), field_name, "", &mut suffix)?;
        if remainder.starts_with('?') {
            return apply_string_formatter_chain(result.unwrap_or_default(), remainder);
        }
        if remainder.starts_with('J') {
            return apply_string_formatter_chain(result.unwrap_or_default(), suffix);
        }
        if !suffix.is_empty() {
            return Err(FormatError(
                "unsupported list arithmetic format suffix".into(),
            ));
        }
        result
    };
    result.ok_or_else(|| FormatError("list arithmetic produced no filename value".into()))
}

fn apply_string_formatter_chain(mut value: String, mut spec: &str) -> Result<String, FormatError> {
    let mut wrappers = Vec::new();
    loop {
        if spec.starts_with('C') {
            let (conversions, remainder) =
                conversion_formatter_spec(spec).expect("conversion formatter starts with C");
            value = convert_string_chain(&value, conversions)?;
            spec = remainder;
        } else if spec.starts_with('R') {
            (value, spec) = apply_chained_replacements(value, spec)?;
        } else if spec.starts_with('X') || spec.starts_with('L') {
            (value, spec) =
                format_string_length(&value, spec)?.expect("length formatter starts with X or L");
        } else if let Some(wrapper) = spec.strip_prefix('?') {
            let (prefix, rest) = wrapper
                .split_once('/')
                .ok_or_else(|| FormatError("invalid conditional filename format".into()))?;
            let (suffix, remainder) = rest
                .split_once('/')
                .ok_or_else(|| FormatError("invalid conditional filename format".into()))?;
            if !value.is_empty() {
                wrappers.push((prefix.to_owned(), suffix.to_owned()));
            }
            spec = remainder;
        } else if spec.is_empty() {
            return Ok(apply_conditional_wrappers(value, &wrappers));
        } else if let Some(precision) = spec.strip_prefix('.') {
            if precision.is_empty() || !precision.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(FormatError("invalid string precision formatting".into()));
            }
            let precision = precision
                .parse::<usize>()
                .map_err(|_| FormatError("invalid string precision".into()))?;
            if precision > 240 {
                return Err(FormatError("filename string precision is too large".into()));
            }
            let value = value.chars().take(precision).collect();
            return Ok(apply_conditional_wrappers(value, &wrappers));
        } else {
            let value = format_string_width(&value, spec)?;
            return Ok(apply_conditional_wrappers(value, &wrappers));
        }
    }
}

fn apply_conditional_wrappers(mut value: String, wrappers: &[(String, String)]) -> String {
    for (prefix, suffix) in wrappers {
        value = format!("{prefix}{value}{suffix}");
    }
    value
}

fn format_string_width(value: &str, spec: &str) -> Result<String, FormatError> {
    let chars: Vec<char> = spec.chars().collect();
    let (fill, align, width_start) = match chars.as_slice() {
        [align, ..] if matches!(align, '<' | '>' | '^') => (' ', *align, 1),
        [fill, align, ..] if matches!(align, '<' | '>' | '^') => (*fill, *align, 2),
        _ => {
            return Err(FormatError("unsupported arithmetic format chain".into()));
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
    let padding = width.saturating_sub(value.chars().count());
    let (left, right) = match align {
        '<' => (0, padding),
        '^' => (padding / 2, padding - padding / 2),
        _ => (padding, 0),
    };
    Ok(format!(
        "{}{}{}",
        fill.to_string().repeat(left),
        value,
        fill.to_string().repeat(right)
    ))
}

fn format_string_length<'a>(
    value: &str,
    spec: &'a str,
) -> Result<Option<(String, &'a str)>, FormatError> {
    let (truncate, rest) = if let Some(rest) = spec.strip_prefix('X') {
        (true, rest)
    } else if let Some(rest) = spec.strip_prefix('L') {
        (false, rest)
    } else {
        return Ok(None);
    };
    let (max_len, rest) = rest
        .split_once('/')
        .ok_or_else(|| FormatError("invalid filename length formatter".into()))?;
    let (replacement, remainder) = rest
        .split_once('/')
        .ok_or_else(|| FormatError("invalid filename length formatter".into()))?;
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
        return Ok(Some((value.to_owned(), remainder)));
    }
    if !truncate {
        return Ok(Some((replacement.to_owned(), remainder)));
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
    let shortened = if byte_len {
        let prefix = decode_utf8_ignoring_invalid(&value.as_bytes()[..max_len - replacement_len]);
        format!("{prefix}{replacement}")
    } else {
        value
            .chars()
            .take(max_len - replacement_len)
            .chain(replacement.chars())
            .collect()
    };
    Ok(Some((shortened, remainder)))
}

fn metadata_field(
    value: &Value,
    field_name: &str,
    conversion: &str,
    spec: &mut &str,
) -> Result<Option<String>, FormatError> {
    if !matches!(value, Value::String(_)) {
        let converted = match conversion {
            "s" | "r" => Some(python_string_value(value)?),
            "a" => Some(python_ascii_repr_value(value)?),
            "g" => Some(slugify(&python_string_value(value)?)),
            _ => None,
        };
        if let Some(converted) = converted {
            return metadata_field(&Value::String(converted), field_name, "", spec);
        }
        if matches!(value, Value::Object(_)) && conversion == "S" {
            let converted = python_string_value(value)?;
            return metadata_field(&Value::String(converted), field_name, "", spec);
        }
    }
    if !matches!(value, Value::String(_)) && spec.starts_with('C') {
        let (conversions, remainder) =
            conversion_formatter_spec(spec).expect("conversion formatter starts with C");
        let Some(trailing) = conversions.strip_prefix('S') else {
            return Err(FormatError(
                "filename non-string conversion requires the S conversion".into(),
            ));
        };
        let value = formatter_to_string(value)?;
        let converted = if trailing.is_empty() {
            value
        } else {
            convert_string_chain(&value, trailing)?
        };
        *spec = remainder;
        return metadata_field(&Value::String(converted), field_name, conversion, spec);
    }
    match value {
        Value::Null if !conversion.is_empty() => Ok(Some("None".into())),
        Value::Null if conversion.is_empty() && spec.starts_with('?') => Ok(Some(String::new())),
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
        Value::Array(values)
            if values.is_empty() && matches!(conversion, "s" | "r" | "a" | "j") =>
        {
            Ok(Some("[]".into()))
        }
        Value::Array(values)
            if values.is_empty() && conversion.is_empty() && spec.starts_with('?') =>
        {
            Ok(Some(String::new()))
        }
        Value::Array(values) if conversion == "n" => Ok(Some(values.len().to_string())),
        Value::Object(fields)
            if fields.is_empty() && matches!(conversion, "s" | "r" | "a" | "j") =>
        {
            Ok(Some("{}".into()))
        }
        Value::Object(fields)
            if fields.is_empty() && conversion.is_empty() && spec.starts_with('?') =>
        {
            Ok(Some(String::new()))
        }
        Value::Object(fields) if conversion == "n" => Ok(Some(fields.len().to_string())),
        Value::Array(_) | Value::Object(_) if conversion == "j" => {
            Ok(Some(python_json_value(value)))
        }
        Value::String(value) if spec.starts_with('M') => {
            let (_, remainder) = map_formatter_spec(spec).expect("map formatter starts with M");
            *spec = remainder;
            metadata_field(&Value::String(value.clone()), field_name, conversion, spec)
        }
        Value::String(value) if spec.starts_with('C') => {
            let (conversions, remainder) =
                conversion_formatter_spec(spec).expect("conversion formatter starts with C");
            let converted = if let Some((prefix, suffix)) = conversions.split_once('R') {
                if prefix.contains('R') {
                    return Err(FormatError(
                        "filename URL extraction conversion may only occur once".into(),
                    ));
                }
                let source = if prefix.is_empty() {
                    value.clone()
                } else {
                    convert_string_chain(value, prefix)?
                };
                let urls = extract_urls(&source);
                if suffix.is_empty() {
                    Value::Array(urls)
                } else {
                    let mut suffix_chars = suffix.chars();
                    let conversion = suffix_chars.next().ok_or_else(|| {
                        FormatError("filename URL extraction conversion is incomplete".into())
                    })?;
                    let trailing: String = suffix_chars.collect();
                    let value = match conversion {
                        'S' => urls
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", "),
                        's' | 'r' => python_array_repr(&urls)?,
                        'a' => python_array_ascii_repr(&urls)?,
                        'j' => python_json_value(&Value::Array(urls)),
                        'n' => urls.len().to_string(),
                        _ => {
                            return Err(FormatError(format!(
                                "unsupported conversion '{conversion}' after URL extraction"
                            )))
                        }
                    };
                    Value::String(if trailing.is_empty() {
                        value
                    } else {
                        convert_string_chain(&value, &trailing)?
                    })
                }
            } else {
                Value::String(convert_string_chain(value, conversions)?)
            };
            *spec = remainder;
            metadata_field(&converted, field_name, conversion, spec)
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
                        Some(value) => Ok(value.clone()),
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
            let mut sorted = values.clone();
            if sorted.iter().all(Value::is_string) {
                sorted.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
            } else if sorted
                .iter()
                .all(|value| value.is_number() || value.is_boolean())
            {
                let integer_key = |value: &Value| match value {
                    Value::Bool(value) => Some(i128::from(*value)),
                    Value::Number(value) => value
                        .as_i64()
                        .map(i128::from)
                        .or_else(|| value.as_u64().map(i128::from)),
                    _ => None,
                };
                if sorted.iter().all(|value| integer_key(value).is_some()) {
                    sorted.sort_by_key(|value| {
                        integer_key(value).expect("all sort values are integers")
                    });
                } else {
                    let numeric_key = |value: &Value| match value {
                        Value::Bool(value) => Some(f64::from(u8::from(*value))),
                        Value::Number(value) => value.as_f64(),
                        _ => None,
                    };
                    sorted.sort_by(|left, right| {
                        numeric_key(left)
                            .expect("all sort values are numeric")
                            .partial_cmp(&numeric_key(right).expect("all sort values are numeric"))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
            } else {
                return Err(FormatError(format!(
                    "filename sort field '{field_name}' must contain comparable values"
                )));
            }
            if options.contains('d') || options.contains('r') {
                sorted.reverse();
            }
            *spec = remainder;
            metadata_field(&Value::Array(sorted), field_name, conversion, spec)
        }
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
            let joined = values.join(separator);
            *spec = remainder;
            metadata_field(&Value::String(joined), field_name, conversion, spec)
        }
        Value::Number(_) | Value::Bool(_) if spec.starts_with('J') => Err(FormatError(
            "filename join formatter requires a string or string list".into(),
        )),
        Value::Number(value) => Ok(Some(value.to_string())),
        Value::Bool(value) => Ok(Some(if *value { "True" } else { "False" }.into())),
        Value::Array(values) if conversion == "S" => values
            .iter()
            .map(python_string_value)
            .collect::<Result<Vec<_>, _>>()
            .map(|values| Some(values.join(", "))),
        Value::Array(values) => python_array_repr(values).map(Some),
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
    let mut selected_source = None;
    let mut missing_field = None;
    for candidate in &candidates {
        let mut candidate_spec = spec;
        let (base_name, selectors) = split_field_selectors(candidate)?;
        let selector = selectors.first().copied();
        let mut selector_applied_to_container = false;
        let mut candidate_source = None;
        let (field_value, is_literal) = if base_name == "_lit" {
            let FieldSelector::Element(literal) = selector.ok_or_else(|| {
                FormatError("literal filename field requires a bracketed value".into())
            })?
            else {
                return Err(FormatError(
                    "literal filename field requires a bracketed value".into(),
                ));
            };
            if selectors.len() != 1 {
                return Err(FormatError(
                    "literal filename field accepts one bracketed value".into(),
                ));
            }
            if literal.is_empty() {
                return Err(FormatError("literal filename field cannot be empty".into()));
            }
            candidate_source = Some(Value::String(literal.to_owned()));
            let mut value = literal.to_owned();
            if candidate_spec.starts_with('C') {
                value = metadata_field(
                    &Value::String(value.clone()),
                    base_name,
                    conversion,
                    &mut candidate_spec,
                )?
                .ok_or_else(|| FormatError("literal conversion produced no value".into()))?;
            }
            (Some(value), true)
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
                        Some(id) => {
                            candidate_source = Some(Value::from(id));
                            Some(id.to_string())
                        }
                        None => {
                            missing_field.get_or_insert(base_name);
                            None
                        }
                    },
                    _ => match metadata.get(base_name) {
                        Some(value) => {
                            let mut selected = value.clone();
                            for selector in &selectors {
                                selected = select_value(&selected, *selector, base_name)?;
                            }
                            selector_applied_to_container = !selectors.is_empty();
                            let value = if selector_applied_to_container {
                                &selected
                            } else {
                                value
                            };
                            candidate_source = Some(value.clone());
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
        let field_value = match (is_literal, selector_applied_to_container, selector) {
            (true, _, _) | (false, true, _) | (false, false, None) => field_value,
            (false, false, Some(_)) => {
                let mut selected = Value::String(field_value);
                for selector in &selectors {
                    selected = select_value(&selected, *selector, base_name)?;
                }
                match selected {
                    Value::String(value) => value,
                    _ => {
                        return Err(FormatError(
                            "filename element selector must produce a string".into(),
                        ))
                    }
                }
            }
        };
        if !field_value.is_empty() && value.is_none() {
            selected_base = Some(base_name);
            selected_spec = Some(candidate_spec);
            selected_source = candidate_source.clone();
            value = Some(field_value);
        } else if field_value.is_empty() && value.is_none() {
            empty_fallback.get_or_insert((
                base_name,
                field_value,
                candidate_spec,
                candidate_source,
            ));
        }
    }
    let (base_name, value) = match (selected_base, value, empty_fallback) {
        (Some(base), Some(value), _) => (base, value),
        (None, None, Some((base, value, spec, source))) => {
            selected_spec = Some(spec);
            selected_source = source;
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
    let native_scalar = selected_source
        .as_ref()
        .filter(|source| matches!(source, Value::Null | Value::Bool(_) | Value::Number(_)));
    let native_non_string = selected_source
        .as_ref()
        .filter(|source| !matches!(source, Value::String(_)));
    if native_scalar.is_some()
        && matches!(
            conversion,
            "l" | "u" | "c" | "C" | "t" | "W" | "q" | "Q" | "U" | "H" | "L"
        )
    {
        return Err(FormatError(
            "filename string conversion requires a string value".into(),
        ));
    }
    let converted_integer = if conversion == "i" {
        Some(python_integer_value(selected_source.as_ref(), &value)?)
    } else {
        None
    };
    let converted_float = if conversion == "f" {
        Some(python_float_value(selected_source.as_ref(), &value)?)
    } else {
        None
    };
    let converted_length = if conversion == "n" {
        Some(python_length_value(selected_source.as_ref(), &value)?)
    } else {
        None
    };
    if conversion == "d" || conversion == "D" {
        return Ok(crate::filename_datetime::convert_datetime(
            &value,
            selected_source.as_ref(),
            spec,
            conversion == "D",
        ));
    }
    let mut value = match conversion {
        "" | "s" | "S" => value,
        "T" => String::new(),
        "r" if native_non_string.is_some() || matches!(base_name, "id" | "post_id") => value,
        "r" => python_string_repr(&value, false),
        "a" if native_non_string.is_some() || matches!(base_name, "id" | "post_id") => value,
        "a" => python_string_repr(&value, true),
        "j" if native_non_string.is_some() => python_json_value(native_non_string.unwrap()),
        "j" => python_json_string(&value),
        "g" => slugify(&value),
        "q" => encode_url_component(&value),
        "Q" => decode_url_component(&value),
        "U" => decode_html_entities(&value),
        "H" => html_to_text(&value),
        "L" => language_name(&value).map(str::to_owned).ok_or_else(|| {
            FormatError("language conversion produces a non-string filename value".into())
        })?,
        "l" => value.to_lowercase(),
        "u" => value.to_uppercase(),
        "c" => python_capitalize(&value),
        "C" => python_capwords(&value),
        "t" => value.trim().to_owned(),
        "W" => value.split_whitespace().collect::<Vec<_>>().join(" "),
        "i" => converted_integer
            .expect("integer conversion has a parsed value")
            .to_string(),
        "f" => converted_float
            .expect("float conversion has a parsed value")
            .to_string(),
        "n" => converted_length
            .expect("length conversion has a computed value")
            .to_string(),
        _ => {
            return Err(FormatError(format!(
                "unsupported conversion '{conversion}'"
            )))
        }
    };
    if conversion == "i" && !spec.starts_with('A') {
        if let Some(formatted) = format_integer_character(&value, spec)? {
            return Ok(formatted);
        }
        if let Some(formatted) = format_integer(&value, spec)? {
            return Ok(formatted);
        }
        return Err(FormatError(
            "integer conversion requires a numeric filename format".into(),
        ));
    }
    if conversion == "f" && !spec.starts_with('A') {
        let number = Value::from(converted_float.expect("float conversion has a parsed value"));
        if let Some(formatted) = format_fixed_numeric(&number, spec)? {
            return Ok(formatted);
        }
        return Err(FormatError(
            "float conversion requires a numeric filename format".into(),
        ));
    }
    if conversion == "n" && !spec.starts_with('A') {
        if let Some(formatted) = format_integer_character(&value, spec)? {
            return Ok(formatted);
        }
        if let Some(formatted) = format_integer(&value, spec)? {
            return Ok(formatted);
        }
        return Err(FormatError(
            "length conversion requires a numeric filename format".into(),
        ));
    }
    if spec.starts_with('D') {
        if conversion.is_empty()
            && (native_non_string.is_some() || matches!(base_name, "id" | "post_id"))
        {
            return Ok("[Invalid DateTime]".into());
        }
        return crate::filename_datetime::format_datetime(&value, spec)
            .ok_or_else(|| FormatError("invalid filename datetime formatter".into()));
    }
    // gallery-dl's identity formatter returns its input directly, ignoring
    // any remaining specifier text.
    if spec.starts_with('I') {
        return Ok(value);
    }
    (value, spec) = apply_chained_replacements(value, spec)?;
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
    if spec.starts_with('X') || spec.starts_with('L') {
        (value, spec) =
            format_string_length(&value, spec)?.expect("length formatter starts with X or L");
        if spec.is_empty() {
            return Ok(value);
        }
    }
    let arithmetic_spec = spec.starts_with('A');
    if !arithmetic_spec
        && (conversion == "i"
            || conversion == "n"
            || matches!(base_name, "id" | "post_id")
            || (conversion.is_empty()
                && selected_source
                    .as_ref()
                    .is_some_and(|source| source.is_number() || source.is_boolean())))
    {
        let integer_value = match selected_source.as_ref() {
            Some(Value::Bool(true)) => "1",
            Some(Value::Bool(false)) => "0",
            _ => &value,
        };
        if conversion.is_empty() || conversion == "i" || conversion == "n" {
            if let Some(formatted) = format_integer_character(integer_value, spec)? {
                return Ok(formatted);
            }
        }
        if let Some(formatted) = format_integer(integer_value, spec)? {
            return Ok(formatted);
        }
    }
    if conversion.is_empty() && !arithmetic_spec {
        if let Some(source) = selected_source.as_ref() {
            if let Some(formatted) = format_fixed_numeric(source, spec)? {
                return Ok(formatted);
            }
        }
    }
    if spec.starts_with('X') || spec.starts_with('L') {
        return Err(FormatError("invalid filename length formatter".into()));
    }
    if spec.starts_with('A') {
        let (operator, _, _) = arithmetic_operation(spec)?;
        if conversion.is_empty() {
            if let Some(Value::Array(values)) = selected_source.as_ref() {
                return apply_array_arithmetic_chain(values, base_name, spec);
            }
        }
        let string_source = !matches!(base_name, "id" | "post_id")
            && selected_source
                .as_ref()
                .is_none_or(|source| matches!(source, Value::String(_)));
        if matches!(
            conversion,
            "s" | "S"
                | "r"
                | "a"
                | "j"
                | "g"
                | "q"
                | "Q"
                | "U"
                | "H"
                | "L"
                | "l"
                | "u"
                | "c"
                | "C"
                | "t"
                | "W"
                | "T"
        ) || (conversion.is_empty() && string_source)
        {
            return apply_string_arithmetic_chain(value, spec);
        }
        let float_left = if conversion == "f" {
            converted_float
        } else {
            selected_source.as_ref().and_then(|source| match source {
                Value::Number(number) if number.is_f64() => number.as_f64(),
                _ => None,
            })
        };
        if let Some(left) = float_left {
            return apply_float_arithmetic_chain(left, spec);
        }
        let left = match selected_source.as_ref() {
            Some(Value::Bool(value)) => i128::from(*value),
            _ => match value.parse::<i128>() {
                Ok(left) => left,
                Err(_) if !matches!(operator, "+" | "-") => {
                    return apply_string_arithmetic_chain(value, spec);
                }
                Err(_) => {
                    return Err(FormatError(
                        "filename arithmetic requires an integer field".into(),
                    ))
                }
            },
        };
        return apply_integer_arithmetic_chain(left, spec);
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
    if spec.len() > 1
        && spec.starts_with('0')
        && spec[1..].bytes().all(|byte| byte.is_ascii_digit())
    {
        let width = spec[1..]
            .parse::<usize>()
            .map_err(|_| FormatError("invalid string width formatting".into()))?;
        if width > 240 {
            return Err(FormatError("filename string width is too large".into()));
        }
        let padding = width.saturating_sub(value.chars().count());
        return Ok(format!("{value}{}", "0".repeat(padding)));
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
        assert_eq!(render("{filename:03}", &item).unwrap(), "original");
        assert!(render("{filename:>241}", &item).is_err());
        assert_eq!(render("{id:D%Y/%Y}", &item).unwrap(), "[Invalid DateTime]");
    }

    #[test]
    fn integer_format_alignment_supports_space_and_custom_fill() {
        let item = sample();
        assert_eq!(field("id:>6d", &item).unwrap(), "    42");
        assert_eq!(field("id:<6d", &item).unwrap(), "42    ");
        assert_eq!(field("id:^6d", &item).unwrap(), "  42  ");
        assert_eq!(field("id:>06d", &item).unwrap(), "000042");
        assert_eq!(field("id:^06d", &item).unwrap(), "004200");
        assert_eq!(field("id:=06d", &item).unwrap(), "000042");
        assert_eq!(field("id:x=6d", &item).unwrap(), "xxxx42");
        assert_eq!(field("id:x>6d", &item).unwrap(), "xxxx42");
        assert_eq!(field("id:x<06d", &item).unwrap(), "42xxxx");
        let metadata = serde_json::json!({"negative": -42});
        let metadata = metadata.as_object().unwrap();
        assert_eq!(
            field_with_metadata("negative:=06d", &item, metadata).unwrap(),
            "-00042"
        );
        assert_eq!(
            field_with_metadata("negative:x=6d", &item, metadata).unwrap(),
            "-xxx42"
        );
    }

    #[test]
    fn integer_character_formats_support_width_fill_and_alignment() {
        let item = sample();
        let metadata = serde_json::json!({"letter": 65});
        let metadata = metadata.as_object().unwrap();
        for (format, expected) in [
            ("letter:c", "A"),
            ("letter:04c", "000A"),
            ("letter:<4c", "A   "),
            ("letter:>4c", "   A"),
            ("letter:x^4c", "xAxx"),
        ] {
            assert_eq!(
                field_with_metadata(format, &item, metadata).unwrap(),
                expected
            );
        }
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
    fn integer_conversion_formats_string_numeric_and_boolean_metadata() {
        let item = sample();
        let metadata = serde_json::from_value::<Metadata>(serde_json::json!({
            "count": 42,
            "numeric_text": " 17 ",
            "underscored": "-1_234",
            "decimal": 12.9,
            "enabled": true,
            "rating_text": "1.2345",
            "grouped_rating": "1_234.5_6",
            "invalid": "not a number",
            "invalid_separator": "1__2"
        }))
        .unwrap();
        assert_eq!(
            render_with_metadata("{count!i:04d}", &item, &metadata).unwrap(),
            "0042"
        );
        assert_eq!(
            render_with_metadata("{numeric_text!i:04d}", &item, &metadata).unwrap(),
            "0017"
        );
        assert_eq!(
            render_with_metadata("{underscored!i:,}", &item, &metadata).unwrap(),
            "-1,234"
        );
        assert_eq!(
            render_with_metadata("{decimal!i:d}", &item, &metadata).unwrap(),
            "12"
        );
        assert_eq!(
            render_with_metadata("{enabled!i:02d}", &item, &metadata).unwrap(),
            "01"
        );
        assert_eq!(
            render_with_metadata("{decimal!f:.1f}", &item, &metadata).unwrap(),
            "12.9"
        );
        assert_eq!(
            render_with_metadata("{rating_text!f:.2f}", &item, &metadata).unwrap(),
            "1.23"
        );
        assert_eq!(
            render_with_metadata("{grouped_rating!f:.2f}", &item, &metadata).unwrap(),
            "1234.56"
        );
        assert!(render_with_metadata("{invalid!i}", &item, &metadata).is_err());
        assert!(render_with_metadata("{invalid_separator!i:04d}", &item, &metadata).is_err());
        assert!(render_with_metadata("{invalid!f:.2f}", &item, &metadata).is_err());
        assert!(render_with_metadata("{count!i}", &item, &metadata).is_err());
        assert!(render_with_metadata("{count!f}", &item, &metadata).is_err());
        assert!(render_with_metadata("{invalid_separator!f:.2f}", &item, &metadata).is_err());
    }

    #[test]
    fn length_conversion_requires_and_accepts_numeric_filename_formats() {
        let item = sample();
        let metadata = serde_json::from_value::<Metadata>(serde_json::json!({
            "title": "猫a",
            "tags": ["first", "second", "third"],
            "labels": {"one": 1, "two": 2},
            "count": 42
        }))
        .unwrap();
        assert_eq!(
            render_with_metadata("{title!n:03d}", &item, &metadata).unwrap(),
            "002"
        );
        assert_eq!(
            render_with_metadata("{tags!n:02d}", &item, &metadata).unwrap(),
            "03"
        );
        assert_eq!(
            render_with_metadata("{labels!n:02d}", &item, &metadata).unwrap(),
            "02"
        );
        assert_eq!(
            render_with_metadata("{tags!n:A+2/02d}", &item, &metadata).unwrap(),
            "05"
        );
        assert_eq!(render("{_lit[猫]!n:02d}", &item).unwrap(), "01");
        assert!(render_with_metadata("{title!n}", &item, &metadata).is_err());
        assert!(render_with_metadata("{count!n:02d}", &item, &metadata).is_err());
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
        assert_eq!(
            render_with_metadata("{tags}", &item, &metadata).unwrap(),
            "['autumn', 'night sky']"
        );
        assert!(render_with_metadata("{author}", &item, &metadata).is_err());
    }

    #[test]
    fn list_and_string_formatter_specs_compose() {
        let item = sample();
        let metadata: Metadata = serde_json::from_value(serde_json::json!({
            "tags": ["zeta", "alpha", "middle"],
            "mixed_tags": ["alpha", 2, false, null, [1, true], {"name": "one"}],
            "json_values": [1, true, null],
            "json_object": {"name": "é"},
            "ordered_object": {"second": 2, "first": 1},
            "title": "plain",
            "display": "  Mixed Case  ",
            "display_encoded": "Mixed%20Case",
            "display_ws": "one\t  TWO\nthree",
            "enabled": false,
            "count": 0,
            "positive": 2,
            "nullable": null,
            "empty_list": [],
            "empty_map": {},
            "empty_string": "",
            "artists": [{"name": "one"}, {"name": "two"}, {"name": "three"}]
        }))
        .unwrap();
        assert_eq!(
            render_with_metadata("{title:08}", &item, &metadata).unwrap(),
            "plain000"
        );
        assert_eq!(
            render_with_metadata("{tags:J, /}", &item, &metadata).unwrap(),
            "zeta, alpha, middle"
        );
        assert_eq!(
            render_with_metadata("{tags:J/}", &item, &metadata).unwrap(),
            "zetaalphamiddle"
        );
        assert_eq!(
            render_with_metadata("{tags:CS/}", &item, &metadata).unwrap(),
            "zeta, alpha, middle"
        );
        assert_eq!(
            render_with_metadata("file{enabled:CS/}", &item, &metadata).unwrap(),
            "file"
        );
        assert_eq!(
            render_with_metadata("file{count:CS/}", &item, &metadata).unwrap(),
            "file"
        );
        assert_eq!(
            render_with_metadata("{positive:CS/}", &item, &metadata).unwrap(),
            "2"
        );
        assert_eq!(
            render_with_metadata("file{nullable:CS/}", &item, &metadata).unwrap(),
            "file"
        );
        assert_eq!(
            formatter_to_string(metadata.get("mixed_tags").unwrap()).unwrap(),
            "alpha, 2, False, None, [1, True], {'name': 'one'}"
        );
        assert_eq!(
            render_with_metadata("{mixed_tags:CSg/}", &item, &metadata).unwrap(),
            "alpha-2-false-none-1-true-name-one"
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
            field_with_metadata("mixed_tags!S", &item, &metadata).unwrap(),
            "alpha, 2, False, None, [1, True], {'name': 'one'}"
        );
        assert_eq!(
            render_with_metadata("{mixed_tags!S:R:/-/}", &item, &metadata).unwrap(),
            "alpha, 2, False, None, [1, True], {'name'- 'one'}"
        );
        assert_eq!(
            render_with_metadata("{json_values!j}", &item, &metadata).unwrap(),
            "[1,true,null]"
        );
        assert_eq!(
            field_with_metadata("json_object!j", &item, &metadata).unwrap(),
            "{\"name\":\"\\u00e9\"}"
        );
        assert_eq!(
            field_with_metadata("ordered_object!j", &item, &metadata).unwrap(),
            "{\"second\":2,\"first\":1}"
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
        let links: Metadata = serde_json::from_value(serde_json::json!({
            "link_text": "Visit https://a.test/x and https://b.test/y"
        }))
        .unwrap();
        assert_eq!(
            render_with_metadata("{link_text:CR/J, /Cg/}", &item, &links).unwrap(),
            "httpsatestx-httpsbtesty"
        );
        assert_eq!(
            render_with_metadata("{link_text:CRS/Cg/}", &item, &links).unwrap(),
            "httpsatestx-httpsbtesty"
        );
        assert_eq!(
            render_with_metadata("{link_text:CRn/}", &item, &links).unwrap(),
            "2"
        );
        assert!(render_with_metadata("{link_text:CR/}", &item, &links).is_err());
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
        assert_eq!(
            render_with_metadata("file{nullable:?yes/no/}", &item, &metadata).unwrap(),
            "file"
        );
        assert_eq!(
            render_with_metadata("file{nullable!s:?yes/no/}", &item, &metadata).unwrap(),
            "fileyesNoneno"
        );
        for (field, expected) in [
            ("enabled!r", "False"),
            ("count!r", "0"),
            ("nullable!r", "None"),
            ("enabled!a", "False"),
            ("count!a", "0"),
            ("nullable!a", "None"),
            ("enabled!j", "false"),
            ("count!j", "0"),
            ("nullable!j", "null"),
            ("id!j", "42"),
        ] {
            assert_eq!(
                render_with_metadata(&format!("{{{field}}}"), &item, &metadata).unwrap(),
                expected
            );
        }
        assert_eq!(
            render_with_metadata("file{nullable!r:?yes/no/}", &item, &metadata).unwrap(),
            "fileyesNoneno"
        );
        for field in ["empty_list", "empty_map", "empty_string"] {
            assert_eq!(
                render_with_metadata(&format!("file{{{field}:?yes/no/}}"), &item, &metadata)
                    .unwrap(),
                "file"
            );
        }
        assert_eq!(
            render_with_metadata("{nullable!g}", &item, &metadata).unwrap(),
            "none"
        );
        assert_eq!(
            render_with_metadata("{enabled!g}", &item, &metadata).unwrap(),
            "false"
        );
        for (field, expected) in [
            ("empty_list!s", "[]"),
            ("empty_list!r", "[]"),
            ("empty_list!a", "[]"),
            ("empty_list!j", "[]"),
            ("empty_map!s", "{}"),
            ("empty_map!r", "{}"),
            ("empty_map!a", "{}"),
            ("empty_map!j", "{}"),
        ] {
            assert_eq!(
                field_with_metadata(field, &item, &metadata).unwrap(),
                expected
            );
        }
        assert_eq!(
            render_with_metadata("file{empty_list!s:?yes/no/}", &item, &metadata).unwrap(),
            "fileyes[]no"
        );
        assert_eq!(
            render_with_metadata("file{empty_map!s:?yes/no/}", &item, &metadata).unwrap(),
            "fileyes{}no"
        );
        assert!(render_with_metadata("{id:J, /}", &item, &metadata).is_err());
        assert!(render_with_metadata("{tags[3]:J, /}", &item, &metadata).is_err());
    }

    #[test]
    fn url_extraction_conversion_uses_pinned_delimiters() {
        assert_eq!(
            extract_urls(
                r#"pre http://a/x?y=1&z=2" https://b.test/path<tail> http://c\rest ftp://d.test"#
            ),
            [
                Value::String("http://a/x?y=1&z=2".into()),
                Value::String("https://b.test/path".into()),
                Value::String("http://c".into()),
            ]
        );
        assert!(extract_urls("text without a URL").is_empty());
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
    fn named_fields_select_mapping_keys() {
        let item = sample();
        let metadata: Metadata = serde_json::from_value(serde_json::json!({
            "labels": {"short": "safe", "display": "indigo"}
        }))
        .unwrap();

        assert_eq!(
            render_with_metadata(
                "{labels['short']}_{labels[short]}_{labels[display]!u}",
                &item,
                &metadata
            )
            .unwrap(),
            "safe_safe_INDIGO"
        );
        assert!(render_with_metadata("{labels[missing]}", &item, &metadata).is_err());
    }

    #[test]
    fn named_fields_chain_mapping_and_array_selectors() {
        let item = MediaItem {
            url: "https://example.org/image.jpg".into(),
            filename: "image.jpg".into(),
            extractor: "directlink",
            post_id: Some(42),
        };
        let metadata: Metadata = serde_json::from_value(serde_json::json!({
            "labels": {"nested": {"name": "nested"}, "short": "safe"},
            "artists": [{"name": "one"}, {"name": "two"}]
        }))
        .unwrap();

        assert_eq!(
            render_with_metadata(
                "{labels[nested][name]}_{labels.nested.name}_{artists[1][name]}_{labels[short][1]}",
                &item,
                &metadata
            )
            .unwrap(),
            "nested_nested_two_a"
        );
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
        item.filename = "path/?:@&=+$,#'! ~*().jpg".into();
        assert_eq!(
            field("filename!q", &item).unwrap(),
            "path/%3F%3A%40%26%3D%2B%24%2C%23%27%21%20~%2A%28%29"
        );
        assert_eq!(
            convert_string_chain("path/?:@&=+$,#'! ~*()", "q").unwrap(),
            "path/%3F%3A%40%26%3D%2B%24%2C%23%27%21%20~%2A%28%29"
        );
        assert_eq!(encode_url_component("/a b"), "/a%20b");
        item.filename = "caf%C3%A9%20tea.jpg".into();
        assert_eq!(render("{stem!Q}", &item).unwrap(), "café tea");
        item.filename = "%FF.jpg".into();
        assert_eq!(render("{stem!Q}", &item).unwrap(), "�");
        assert_eq!(convert_string_chain("%FF", "Q").unwrap(), "�");
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
    fn slug_conversion_preserves_words_and_normalizes_separators() {
        let item = sample();
        assert_eq!(field("_lit[Hello, World]!g", &item).unwrap(), "hello-world");
        assert_eq!(field("_lit[foo,bar]!g", &item).unwrap(), "foobar");
        assert_eq!(
            field("_lit[Foo__BAR--baz]!g", &item).unwrap(),
            "foo__bar-baz"
        );
        assert_eq!(field("_lit[猫 World]!g", &item).unwrap(), "猫-world");
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
            "foo__bar-baz"
        );
        assert_eq!(field("_lit[__foo__]!g", &item).unwrap(), "foo");
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
        assert_eq!(render("{id:A+8/04d}", &item).unwrap(), "0050");
        assert_eq!(render("{id:A+1/A*2/}", &item).unwrap(), "86");
        assert_eq!(render("{id:A+1/A*2/04d}", &item).unwrap(), "0086");
        assert_eq!(render("{id:A+1/CS/R4/9/}", &item).unwrap(), "93");
        assert!(render("{filename:A+1/}", &item).is_err());
        assert!(render("{id:A/}", &item).is_err());
        assert!(render("{id:A/2/}", &item).is_err());
        let mut large = item;
        large.post_id = Some(u64::MAX);
        assert!(render("{id:A*170141183460469231731687303715884105727/}", &large).is_err());
    }

    #[test]
    fn floating_point_arithmetic_preserves_numeric_type() {
        let item = sample();
        let metadata: Metadata = serde_json::from_value(serde_json::json!({
            "rating": 1.25,
            "whole": 1.0,
            "ratio": 0.125,
            "count": 42,
            "text_number": "1.25",
            "yes": true,
            "no": false,
            "text": "ab"
        }))
        .unwrap();
        assert_eq!(
            render_with_metadata("{rating:A*2/}", &item, &metadata).unwrap(),
            "2.5"
        );
        assert_eq!(
            render_with_metadata("{rating:A+1/.2f}", &item, &metadata).unwrap(),
            "2.25"
        );
        assert_eq!(
            render_with_metadata("{rating:A*2/A+1/}", &item, &metadata).unwrap(),
            "3.5"
        );
        assert_eq!(
            render_with_metadata("{rating:A*2/CS/R2/7/}", &item, &metadata).unwrap(),
            "7.5"
        );
        assert_eq!(
            render_with_metadata("{whole:A*2/}", &item, &metadata).unwrap(),
            "2.0"
        );
        assert_eq!(
            render_with_metadata("{ratio:A*2/}", &item, &metadata).unwrap(),
            "0.25"
        );
        assert_eq!(
            render_with_metadata("{count!f:A*2/}", &item, &metadata).unwrap(),
            "84.0"
        );
        assert_eq!(
            render_with_metadata("{text_number!f:A*2/}", &item, &metadata).unwrap(),
            "2.5"
        );
        assert_eq!(
            render_with_metadata("{yes:A+1/}", &item, &metadata).unwrap(),
            "2"
        );
        assert_eq!(
            render_with_metadata("{yes:A*2/}", &item, &metadata).unwrap(),
            "2"
        );
        assert_eq!(
            render_with_metadata("{no:A*2/}", &item, &metadata).unwrap(),
            "0"
        );
        assert_eq!(
            render_with_metadata("{text:A*2/}", &item, &metadata).unwrap(),
            "abab"
        );
        assert_eq!(
            render_with_metadata("{text:A*2/A*2/}", &item, &metadata).unwrap(),
            "abababab"
        );
        assert_eq!(
            render_with_metadata("{text:A*2/Rab/x/}", &item, &metadata).unwrap(),
            "xx"
        );
        assert_eq!(
            render_with_metadata("{text:A*2/Cu/}", &item, &metadata).unwrap(),
            "ABAB"
        );
        assert_eq!(
            render_with_metadata("{text:A*2/Cu/RB/X/}", &item, &metadata).unwrap(),
            "AXAX"
        );
        assert_eq!(
            render_with_metadata("{text:A*2/Cu/Cc/}", &item, &metadata).unwrap(),
            "Abab"
        );
        assert_eq!(
            render_with_metadata("{text:A*2/Cu/RB/X/>8}", &item, &metadata).unwrap(),
            "    AXAX"
        );
        assert_eq!(
            render_with_metadata("{text:A*2/Cu/X3/!/}", &item, &metadata).unwrap(),
            "AB!"
        );
        assert_eq!(
            render_with_metadata("{text:A*2/Cu/?[/]/>8}", &item, &metadata).unwrap(),
            "[    ABAB]"
        );
        assert_eq!(
            render_with_metadata("{text:A*2/Cu/.3}", &item, &metadata).unwrap(),
            "ABA"
        );
    }

    #[test]
    fn floating_point_standard_formatting_applies_precision_and_zero_width() {
        let item = sample();
        let metadata: Metadata = serde_json::from_value(serde_json::json!({
            "rating": 1.2345,
            "whole": 42,
        }))
        .unwrap();
        assert_eq!(
            render_with_metadata("{rating:.2f}", &item, &metadata).unwrap(),
            "1.23"
        );
        assert_eq!(
            render_with_metadata("{rating:f}", &item, &metadata).unwrap(),
            "1.234500"
        );
        assert_eq!(
            render_with_metadata("{rating:08.2f}", &item, &metadata).unwrap(),
            "00001.23"
        );
        assert_eq!(
            render_with_metadata("{whole:.2f}", &item, &metadata).unwrap(),
            "42.00"
        );
        assert_eq!(
            render_with_metadata("{whole:.0f}", &item, &metadata).unwrap(),
            "42"
        );
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
        assert_eq!(render("{id:#,}", &item).unwrap(), "1,234,567");
        assert_eq!(render("{id:012,}", &item).unwrap(), "0,001,234,567");
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
    fn html_conversion_decodes_named_and_numeric_entities() {
        assert_eq!(
            decode_html_entities("&lt;p&gt;foo &amp; &#x62;ar&lt;/p&gt;"),
            "<p>foo & bar</p>"
        );
        assert_eq!(
            decode_html_entities("unmatched &amp text"),
            "unmatched & text"
        );
        assert_eq!(decode_html_entities("&amp;amp;"), "&amp;");
        assert_eq!(decode_html_entities("&copy &notin &CoPy"), "© ¬in &CoPy");
        assert_eq!(decode_html_entities("&#128 &#x80; &#0;"), "€ € �");
        assert_eq!(decode_html_entities("&#1;|&#xFDD0;|&#xD800;"), "||�");
    }

    #[test]
    fn html_and_language_conversions_can_be_chained() {
        let item = sample();
        assert_eq!(render("{_lit[a &amp; b]:CUl/}", &item).unwrap(), "a & b");
        assert_eq!(
            render("{_lit[<p>a &amp; <b>b</b></p>]:CHu/}", &item).unwrap(),
            "A & B"
        );
        assert_eq!(render("{_lit[en]:CLu/}", &item).unwrap(), "ENGLISH");
    }

    #[test]
    fn html_text_conversion_removes_tags_and_collapses_whitespace() {
        assert_eq!(
            html_to_text("<p>  foo &amp; <b>bar</b> </p><!-- ignored --> baz"),
            "foo & bar baz"
        );
        assert_eq!(html_to_text("<a title=\"1 > 0\">link</a>"), "link");
        assert_eq!(html_to_text("plain text"), "plain text");
    }

    #[test]
    fn language_conversion_maps_reference_codes_case_insensitively() {
        for (code, name) in [
            ("ar", "Arabic"),
            ("bg", "Bulgarian"),
            ("bn", "Bengali"),
            ("ca", "Catalan"),
            ("cs", "Czech"),
            ("da", "Danish"),
            ("de", "German"),
            ("el", "Greek"),
            ("en", "English"),
            ("es", "Spanish"),
            ("fa", "Persian"),
            ("fi", "Finnish"),
            ("fr", "French"),
            ("he", "Hebrew"),
            ("hi", "Hindi"),
            ("hu", "Hungarian"),
            ("id", "Indonesian"),
            ("it", "Italian"),
            ("ja", "Japanese"),
            ("ko", "Korean"),
            ("ms", "Malay"),
            ("nl", "Dutch"),
            ("no", "Norwegian"),
            ("pl", "Polish"),
            ("pt", "Portuguese"),
            ("ro", "Romanian"),
            ("ru", "Russian"),
            ("sk", "Slovak"),
            ("sl", "Slovenian"),
            ("sr", "Serbian"),
            ("sv", "Swedish"),
            ("th", "Thai"),
            ("tr", "Turkish"),
            ("uk", "Ukrainian"),
            ("vi", "Vietnamese"),
            ("zh", "Chinese"),
        ] {
            assert_eq!(language_name(code), Some(name));
            assert_eq!(language_name(&code.to_ascii_uppercase()), Some(name));
        }
        assert_eq!(language_name("xx"), None);
        assert!(render("{_lit[xx]!L}", &sample()).is_err());
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
        assert_eq!(render("{filename:02}", &item).unwrap(), "original");
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
