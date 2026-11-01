//! Bounded request delay values and shared pacing for the CLI preview.
use serde_json::Value;
use std::{
    collections::HashMap,
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_SLEEP_SECONDS: f64 = 3600.0;
static SAMPLE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static REQUEST_SCHEDULER: OnceLock<tokio::sync::Mutex<RequestSchedule>> = OnceLock::new();

tokio::task_local! {
    static REQUEST_DELAY: DelayRange;
    static REQUEST_HOST: String;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateRange {
    minimum: u64,
    maximum: u64,
}

impl RateRange {
    pub fn parse(input: &str) -> Result<Self, &'static str> {
        let (minimum, maximum) = match input.split_once('-') {
            Some((minimum, maximum)) => (
                parse_bytes_per_second(minimum)?,
                parse_bytes_per_second(maximum)?,
            ),
            None => {
                let value = parse_bytes_per_second(input)?;
                (value, value)
            }
        };
        if minimum > maximum {
            return Err("minimum download rate must not exceed maximum download rate");
        }
        Ok(Self { minimum, maximum })
    }

    pub fn sample(self) -> u64 {
        if self.minimum == self.maximum {
            return self.minimum;
        }
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

        let minimum = u128::from(self.minimum);
        let span = u128::from(self.maximum) - minimum + 1;
        (minimum + u128::from(random) % span) as u64
    }
}

fn parse_bytes_per_second(input: &str) -> Result<u64, &'static str> {
    let input = input.trim();
    let number_end = input
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .unwrap_or(input.len());
    let number = input[..number_end]
        .parse::<f64>()
        .map_err(|_| "invalid download rate")?;
    let multiplier = match input[number_end..].to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        _ => return Err("invalid download rate suffix"),
    };
    let bytes_per_second = number * multiplier;
    if !bytes_per_second.is_finite()
        || bytes_per_second <= 0.0
        || bytes_per_second >= u64::MAX as f64
    {
        return Err("download rate must be a positive finite number of bytes per second");
    }
    let bytes_per_second = bytes_per_second.round() as u64;
    if bytes_per_second == 0 {
        return Err("download rate must be at least one byte per second");
    }
    Ok(bytes_per_second)
}

#[derive(Default)]
struct RequestSchedule {
    global_last: Option<tokio::time::Instant>,
    host_last: HashMap<String, tokio::time::Instant>,
    host_cooldown: HashMap<String, tokio::time::Instant>,
}

pub async fn with_request_delay<F: Future>(delay: Option<DelayRange>, future: F) -> F::Output {
    match delay {
        Some(delay) => REQUEST_DELAY.scope(delay, future).await,
        None => future.await,
    }
}

pub async fn with_request_delay_for_url<F: Future>(
    url: &str,
    delay: Option<DelayRange>,
    future: F,
) -> F::Output {
    let host = request_origin(url);
    match (host, delay) {
        (Some(host), Some(delay)) => {
            REQUEST_HOST
                .scope(host, REQUEST_DELAY.scope(delay, future))
                .await
        }
        (Some(host), None) => REQUEST_HOST.scope(host, future).await,
        (None, Some(delay)) => REQUEST_DELAY.scope(delay, future).await,
        (None, None) => future.await,
    }
}

pub async fn wait_for_request() {
    let delay = REQUEST_DELAY.try_with(|range| range.sample()).ok();
    let host = REQUEST_HOST.try_with(String::clone).ok();
    wait_for_request_in_scope(host, delay).await;
}

pub async fn wait_for_request_to(url: &str) {
    let delay = REQUEST_DELAY.try_with(|range| range.sample()).ok();
    wait_for_request_in_scope(request_origin(url), delay).await;
}

pub async fn wait_for_request_delay(delay: Option<Duration>) {
    wait_for_request_in_scope(None, delay).await;
}

pub async fn wait_for_request_url(url: &str, delay: Option<Duration>) {
    wait_for_request_in_scope(request_origin(url), delay).await;
}

pub async fn defer_host_for_url(url: &str, duration: Duration) {
    let Some(host) = request_origin(url) else {
        return;
    };
    if duration.is_zero() {
        return;
    }
    let scheduler = REQUEST_SCHEDULER.get_or_init(Default::default);
    let mut schedule = scheduler.lock().await;
    let now = tokio::time::Instant::now();
    schedule.host_cooldown.retain(|_, deadline| *deadline > now);
    let deadline = now + duration;
    schedule
        .host_cooldown
        .entry(host)
        .and_modify(|current| *current = (*current).max(deadline))
        .or_insert(deadline);
}

async fn wait_for_request_in_scope(host: Option<String>, delay: Option<Duration>) {
    let delay = delay.filter(|delay| !delay.is_zero());
    if host.is_none() && delay.is_none() {
        return;
    }
    let scheduler = REQUEST_SCHEDULER.get_or_init(Default::default);
    let mut schedule = scheduler.lock().await;
    let now = tokio::time::Instant::now();
    schedule.host_cooldown.retain(|_, deadline| *deadline > now);
    schedule
        .host_last
        .retain(|_, last| delay.is_some_and(|delay| *last + delay > now));

    let deadline = request_deadline(&schedule, host.as_deref(), delay, now);
    tokio::time::sleep_until(deadline).await;
    // Store only after the wait completes. A cancelled future then cannot
    // reserve a stale future slot that delays later requests unnecessarily.
    let admitted = tokio::time::Instant::now();
    if delay.is_some() {
        schedule.global_last = Some(admitted);
        if let Some(host) = host {
            schedule.host_last.insert(host, admitted);
        }
    }
}

fn request_deadline(
    schedule: &RequestSchedule,
    host: Option<&str>,
    delay: Option<Duration>,
    now: tokio::time::Instant,
) -> tokio::time::Instant {
    let global_next = delay.map_or(now, |delay| {
        schedule.global_last.map_or(now, |last| last.max(now)) + delay
    });
    let host_next = host
        .and_then(|host| {
            delay.map(|delay| {
                schedule
                    .host_last
                    .get(host)
                    .map_or(now, |last| (*last).max(now))
                    + delay
            })
        })
        .unwrap_or(now);
    let cooldown = host
        .and_then(|host| schedule.host_cooldown.get(host).copied())
        .unwrap_or(now);
    global_next.max(host_next).max(cooldown).max(now)
}

fn request_origin(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.origin().ascii_serialization())
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
    fn parses_and_samples_bounded_download_rate_ranges() {
        assert_eq!(
            RateRange::parse("8k").unwrap(),
            RateRange {
                minimum: 8 * 1024,
                maximum: 8 * 1024
            }
        );
        let range = RateRange::parse("800k-2M").unwrap();
        assert_eq!(range.minimum, 800 * 1024);
        assert_eq!(range.maximum, 2 * 1024 * 1024);
        for _ in 0..100 {
            assert!((range.minimum..=range.maximum).contains(&range.sample()));
        }
        assert!(RateRange::parse("0").is_err());
        assert!(RateRange::parse("2M-800k").is_err());
        assert!(RateRange::parse("1k-2M-3M").is_err());
    }

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
    fn host_cooldowns_are_scoped_while_global_slots_cover_all_hosts() {
        let now = tokio::time::Instant::now();
        let mut schedule = RequestSchedule::default();
        let throttled = "https://media.example";
        let other = "https://other.example";
        let cooldown = now + Duration::from_secs(2);
        schedule
            .host_cooldown
            .insert(throttled.to_owned(), cooldown);
        assert_eq!(
            request_deadline(&schedule, Some(throttled), None, now),
            cooldown
        );
        assert_eq!(request_deadline(&schedule, Some(other), None, now), now);

        assert_eq!(
            request_deadline(
                &schedule,
                Some(other),
                Some(Duration::from_millis(500)),
                now
            ),
            now + Duration::from_millis(500)
        );

        let host_next = now + Duration::from_millis(500);
        schedule.host_cooldown.clear();
        schedule.host_last.insert(throttled.to_owned(), now);
        assert_eq!(
            request_deadline(
                &schedule,
                Some(throttled),
                Some(Duration::from_millis(500)),
                now
            ),
            host_next
        );
        let global_next = now + Duration::from_secs(1);
        schedule.global_last = Some(now);
        assert_eq!(
            request_deadline(&schedule, Some(other), Some(Duration::from_secs(1)), now),
            global_next
        );
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
