//! Rust implementation of gallery-dl's documented 1-based range syntax.
//! Selection is applied after each currently supported extractor resolves.
use std::{error::Error, fmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeError(pub String);
impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for RangeError {}

#[derive(Debug, Clone)]
struct Clause {
    start: u64,
    end: Option<u64>,
    step: u64,
}

#[derive(Debug, Clone)]
pub struct IndexRange(Vec<Clause>);

fn number(text: &str) -> Result<u64, RangeError> {
    text.trim().parse().map_err(|_| RangeError("invalid numeric range".into()))
}

impl IndexRange {
    pub fn parse(spec: &str) -> Result<Self, RangeError> {
        let mut ranges = Vec::new();
        for segment in spec.split(',').map(str::trim).filter(|part| !part.is_empty()) {
            let (start, end, step) = if segment.contains(':') {
                let parts: Vec<&str> = segment.split(':').collect();
                if parts.len() > 3 { return Err(RangeError("too many slice components".into())); }
                let start = if parts[0].trim().is_empty() { 1 } else { number(parts[0])? };
                let end = if parts[1].trim().is_empty() { None } else { Some(number(parts[1])?) };
                let step = if parts.len() == 3 && !parts[2].trim().is_empty() {
                    number(parts[2])?
                } else { 1 };
                (start, end, step)
            } else if let Some((a, b)) = segment.split_once('-') {
                let start = if a.trim().is_empty() { 1 } else { number(a)? };
                let end = if b.trim().is_empty() {
                    None
                } else {
                    Some(number(b)?.checked_add(1)
                        .ok_or_else(|| RangeError("range limit is too large".into()))?)
                };
                (start, end, 1)
            } else {
                let start = number(segment)?;
                let end = start.checked_add(1)
                    .ok_or_else(|| RangeError("range limit is too large".into()))?;
                (start, Some(end), 1)
            };
            if start == 0 || step == 0 || end.is_some_and(|end| end <= start) {
                return Err(RangeError("range must contain positive ascending indices".into()));
            }
            ranges.push(Clause { start, end, step });
        }
        if ranges.is_empty() {
            return Err(RangeError("range must contain at least one index".into()));
        }
        Ok(Self(ranges))
    }

    pub fn contains(&self, index: usize) -> bool {
        let Ok(index) = u64::try_from(index) else { return false; };
        self.0.iter().any(|range| {
            index >= range.start
                && range.end.is_none_or(|end| index < end)
                && (index - range.start) % range.step == 0
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supports_documented_single_inclusive_and_exclusive_ranges() {
        let selected = IndexRange::parse("-2,4,6-8,10-").unwrap();
        for value in [1, 2, 4, 6, 7, 8, 10, 999] {
            assert!(selected.contains(value), "{value}");
        }
        for value in [3, 5, 9] { assert!(!selected.contains(value), "{value}"); }
        let step = IndexRange::parse("1:7:2").unwrap();
        for value in [1, 3, 5] { assert!(step.contains(value)); }
        for value in [2, 6, 7] { assert!(!step.contains(value)); }
    }

    #[test]
    fn rejects_invalid_and_never_uses_index_zero() {
        for value in ["", "0", "1:8:0", "4-2", "3:2", "a", "1:3:4:2", "1,-1:0"] {
            assert!(IndexRange::parse(value).is_err(), "{value}");
        }
        assert!(!IndexRange::parse("1-").unwrap().contains(0));
    }
}
