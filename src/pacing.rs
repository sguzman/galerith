//! Bounded download delay values used by the CLI pacing preview.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_SLEEP_SECONDS: f64 = 3600.0;
static SAMPLE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
}

fn parse_seconds(input: &str) -> Result<Duration, &'static str> {
    let seconds = input
        .parse::<f64>()
        .map_err(|_| "invalid sleep value: expected seconds or MIN-MAX")?;
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
}
