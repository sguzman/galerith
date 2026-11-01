//! Bounded download delay values used by the CLI pacing preview.
use serde_json::Value;
use std::{
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_SLEEP_SECONDS: f64 = 3600.0;
static SAMPLE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

tokio::task_local! {
    static API_REQUEST_DELAY: DelayRange;
}

pub async fn with_api_request_delay<F: Future>(delay: Option<DelayRange>, future: F) -> F::Output {
    match delay {
        Some(delay) => API_REQUEST_DELAY.scope(delay, future).await,
        None => future.await,
    }
}

pub async fn wait_for_api_request() {
    if let Ok(delay) = API_REQUEST_DELAY.try_with(|range| range.sample()) {
        tokio::time::sleep(delay).await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelayRange {
    minimum: Duration,
    maximum: Duration,
}

impl DelayRange {
    pub fn parse(input: &str) -> Result<Self, &'static str> {
        let (minimum, maximum) = if input.parse::<f64>().is_ok() {
            let value = parse_seconds(input)?;
            (value, value)
        } else {
            let (minimum, maximum) = input
                .split_once('-')
                .ok_or("expected seconds or an inclusive MIN-MAX range")?;
            (parse_seconds(minimum)?, parse_seconds(maximum)?)
        };
        if minimum > maximum {
            return Err("minimum sleep must not exceed maximum sleep");
        }
        Ok(Self { minimum, maximum })
    }

    pub fn sample(self) -> Duration {
        let minimum = self.minimum.as_nanos() as u64;
        let maximum = self.maximum.as_nanos() as u64;
        if minimum == maximum {
            return self.minimum;
        }

        // The preview needs non-cryptographic jitter, not security-sensitive
        // randomness. Mix a per-process sequence with the current clock tick.
        let tick = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos() as u64);
        let sequence = SAMPLE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut random = tick ^ sequence.rotate_left(23) ^ u64::from(std::process::id());
        random ^= random >> 30;
        random = random.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        random ^= random >> 27;
        random = random.wrapping_mul(0x94d0_49bb_1331_11eb);
        random ^= random >> 31;

        let span = maximum - minimum;
        let sampled = minimum + random % (span + 1);
        Duration::from_nanos(sampled)
    }

    pub fn from_value(value: &Value) -> Result<Self, &'static str> {
        match value {
            Value::Number(number) => {
                let seconds = number.as_f64().ok_or("invalid duration number")?;
                let duration = parse_seconds_value(seconds)?;
                Ok(Self {
                    minimum: duration,
                    maximum: duration,
                })
            }
            Value::String(range) => Self::parse(range),
            Value::Array(values) if values.len() == 2 => {
                let minimum = values[0]
                    .as_f64()
                    .ok_or("duration range values must be numbers")?;
                let maximum = values[1]
                    .as_f64()
                    .ok_or("duration range values must be numbers")?;
                Self::from_seconds(minimum, maximum)
            }
            Value::Array(_) => Err("duration array must contain exactly two values"),
            _ => Err("duration must be a number, range string, or two-number array"),
        }
    }

    fn from_seconds(minimum: f64, maximum: f64) -> Result<Self, &'static str> {
        let minimum = parse_seconds_value(minimum)?;
        let maximum = parse_seconds_value(maximum)?;
        if minimum > maximum {
            return Err("minimum sleep must not exceed maximum sleep");
        }
        Ok(Self { minimum, maximum })
    }
}

fn parse_seconds(input: &str) -> Result<Duration, &'static str> {
    let seconds = input
        .parse::<f64>()
        .map_err(|_| "invalid sleep value: expected seconds or MIN-MAX")?;
    parse_seconds_value(seconds)
}

fn parse_seconds_value(seconds: f64) -> Result<Duration, &'static str> {
    if !seconds.is_finite() || !(0.0..=MAX_SLEEP_SECONDS).contains(&seconds) {
        return Err("invalid sleep value: expected 0 to 3600 seconds");
    }
    Ok(Duration::from_secs_f64(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixed_values_and_inclusive_ranges() {
        let fixed = DelayRange::parse("0.15").unwrap();
        assert_eq!(fixed.sample(), Duration::from_millis(150));

        let range = DelayRange::parse("0.1-0.3").unwrap();
        for _ in 0..100 {
            let value = range.sample();
            assert!((Duration::from_millis(100)..=Duration::from_millis(300)).contains(&value));
        }
    }

    #[test]
    fn rejects_invalid_descending_and_unbounded_values() {
        for value in ["", "NaN", "inf", "-1", "0.3-0.1", "0-3600.1", "0.1-0.2-0.3"] {
            assert!(DelayRange::parse(value).is_err(), "accepted {value:?}");
        }
    }

    #[test]
    fn accepts_reference_json_duration_values() {
        for (value, expected) in [
            (serde_json::json!(0.25), DelayRange::parse("0.25").unwrap()),
            (
                serde_json::json!("0.1-0.3"),
                DelayRange::from_seconds(0.1, 0.3).unwrap(),
            ),
            (
                serde_json::json!([0.1, 0.3]),
                DelayRange::from_seconds(0.1, 0.3).unwrap(),
            ),
        ] {
            assert_eq!(DelayRange::from_value(&value).unwrap(), expected);
        }
        for value in [serde_json::json!([0.3, 0.1]), serde_json::json!([0, 1, 2])] {
            assert!(DelayRange::from_value(&value).is_err());
        }
    }
}
