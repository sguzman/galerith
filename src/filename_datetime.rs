//! Bounded, locale-independent date formatting used by filename templates.

#[derive(Clone, Copy, Debug)]
struct DateTime {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    microsecond: u32,
    offset_minutes: Option<i32>,
}

impl DateTime {
    fn valid(self) -> bool {
        (1..=9999).contains(&self.year)
            && (1..=12).contains(&self.month)
            && (1..=days_in_month(self.year, self.month)).contains(&self.day)
            && self.hour < 24
            && self.minute < 60
            && self.second < 60
            && self.microsecond < 1_000_000
    }

    fn normalize_offset(mut self) -> Self {
        self.microsecond = 0;
        if let Some(offset) = self.offset_minutes.take() {
            let minutes = days_from_civil(self.year, self.month, self.day) * 1440
                + i64::from(self.hour) * 60
                + i64::from(self.minute)
                - i64::from(offset);
            let days = minutes.div_euclid(1440);
            let day_minutes = minutes.rem_euclid(1440);
            (self.year, self.month, self.day) = civil_from_days(days);
            self.hour = (day_minutes / 60) as u32;
            self.minute = (day_minutes % 60) as u32;
            self.offset_minutes = Some(0);
        }
        self
    }

    fn add_minutes(mut self, minutes: i64) -> Option<Self> {
        let total = days_from_civil(self.year, self.month, self.day)
            .checked_mul(1440)?
            .checked_add(i64::from(self.hour) * 60 + i64::from(self.minute))?
            .checked_add(minutes)?;
        let days = total.div_euclid(1440);
        let day_minutes = total.rem_euclid(1440);
        (self.year, self.month, self.day) = civil_from_days(days);
        self.hour = (day_minutes / 60) as u32;
        self.minute = (day_minutes % 60) as u32;
        self.valid().then_some(self)
    }

    fn ordinal(self) -> u32 {
        let before_month = (1..self.month)
            .map(|month| days_in_month(self.year, month))
            .sum::<u32>();
        before_month + self.day
    }

    fn weekday(self) -> u32 {
        (days_from_civil(self.year, self.month, self.day) + 4).rem_euclid(7) as u32
    }
}

/// Apply gallery-dl's `D<input-format>/<output-format>` behavior for common
/// `strptime`/`strftime` directives. Unsupported or invalid dates produce the
/// reference formatter's invalid-date marker.
pub(crate) fn format_datetime(value: &str, spec: &str) -> Option<String> {
    let body = spec.strip_prefix('D')?;
    let (input_format, format_spec) = body.split_once('/').unwrap_or((body, ""));
    let date = parse_datetime(value, input_format).map(DateTime::normalize_offset);
    let (date, output_format) = match date {
        Some(date) if format_spec.starts_with('O') => {
            let offset_spec = format_spec.strip_prefix('O').unwrap_or_default();
            let (offset, output_format) = offset_spec.split_once('/').unwrap_or((offset_spec, ""));
            (
                parse_fixed_offset(offset).and_then(|minutes| date.add_minutes(minutes)),
                output_format,
            )
        }
        date => (date, format_spec),
    };
    Some(match date {
        Some(date) if date.valid() => format_datetime_value(date, output_format),
        _ => "[Invalid DateTime]".into(),
    })
}

fn parse_fixed_offset(value: &str) -> Option<i64> {
    if value.is_empty() || value == "local" {
        return None;
    }
    let (hours, minutes) = value.split_once(':').unwrap_or((value, ""));
    let hours = hours.parse::<i64>().ok()?;
    let mut offset = hours.checked_mul(60)?;
    if !minutes.is_empty() {
        let minutes = minutes.parse::<i64>().ok()?;
        let signed_minutes = if offset > 0 {
            minutes
        } else {
            minutes.checked_neg()?
        };
        offset = offset.checked_add(signed_minutes)?;
    }
    (offset.unsigned_abs() <= 60 * 24 * 366).then_some(offset)
}

fn parse_datetime(value: &str, format: &str) -> Option<DateTime> {
    let mut date = DateTime {
        year: 1900,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_minutes: None,
    };
    let mut input = 0;
    let mut chars = format.chars();
    while let Some(character) = chars.next() {
        if character != '%' {
            if character.is_ascii_whitespace() {
                while value
                    .as_bytes()
                    .get(input)
                    .is_some_and(u8::is_ascii_whitespace)
                {
                    input += 1;
                }
                continue;
            }
            if value.get(input..)?.starts_with(character) {
                input += character.len_utf8();
                continue;
            }
            return None;
        }
        let directive = chars.next()?;
        match directive {
            '%' => {
                if value.as_bytes().get(input) != Some(&b'%') {
                    return None;
                }
                input += 1;
            }
            'Y' => date.year = read_number(value, &mut input, 1, 4)? as i32,
            'y' => {
                let year = read_number(value, &mut input, 2, 2)?;
                date.year = if year <= 68 {
                    2000 + year as i32
                } else {
                    1900 + year as i32
                };
            }
            'm' => date.month = read_number(value, &mut input, 1, 2)?,
            'd' | 'e' => date.day = read_number(value, &mut input, 1, 2)?,
            'H' | 'k' => date.hour = read_number(value, &mut input, 1, 2)?,
            'I' | 'l' => {
                let hour = read_number(value, &mut input, 1, 2)?;
                date.hour = hour % 12;
            }
            'M' => date.minute = read_number(value, &mut input, 1, 2)?,
            'S' => date.second = read_number(value, &mut input, 1, 2)?,
            'f' => {
                let start = input;
                let micros = read_number(value, &mut input, 1, 6)?;
                date.microsecond = micros * 10u32.pow(6 - (input - start) as u32);
            }
            'j' => {
                let ordinal = read_number(value, &mut input, 1, 3)?;
                if ordinal == 0 || ordinal > if is_leap_year(date.year) { 366 } else { 365 } {
                    return None;
                }
                let mut month = 1;
                let mut day = ordinal;
                while day > days_in_month(date.year, month) {
                    day -= days_in_month(date.year, month);
                    month += 1;
                }
                date.month = month;
                date.day = day;
            }
            'z' => date.offset_minutes = Some(read_offset(value, &mut input)?),
            'a' | 'A' | 'b' | 'B' | 'p' | 'Z' => {
                let word_start = input;
                while value
                    .as_bytes()
                    .get(input)
                    .is_some_and(u8::is_ascii_alphabetic)
                {
                    input += 1;
                }
                if input == word_start {
                    return None;
                }
                let word = &value[word_start..input];
                match directive {
                    'b' | 'B' => date.month = month_number(word)?,
                    'p' if word.eq_ignore_ascii_case("PM") => date.hour = (date.hour % 12) + 12,
                    'p' if word.eq_ignore_ascii_case("AM") => date.hour %= 12,
                    'p' | 'a' | 'A' | 'Z' => {}
                    _ => unreachable!(),
                }
            }
            _ => return None,
        }
    }
    while value
        .as_bytes()
        .get(input)
        .is_some_and(u8::is_ascii_whitespace)
    {
        input += 1;
    }
    (input == value.len() && date.valid()).then_some(date)
}

fn read_number(value: &str, cursor: &mut usize, min: usize, max: usize) -> Option<u32> {
    while value.as_bytes().get(*cursor) == Some(&b' ') {
        *cursor += 1;
    }
    let start = *cursor;
    while value
        .as_bytes()
        .get(*cursor)
        .is_some_and(u8::is_ascii_digit)
        && *cursor - start < max
    {
        *cursor += 1;
    }
    if *cursor - start < min {
        return None;
    }
    value[start..*cursor].parse().ok()
}

fn read_offset(value: &str, cursor: &mut usize) -> Option<i32> {
    let sign = match value.as_bytes().get(*cursor)? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    *cursor += 1;
    let hours = read_number(value, cursor, 2, 2)? as i32;
    if value.as_bytes().get(*cursor) == Some(&b':') {
        *cursor += 1;
    }
    let minutes = read_number(value, cursor, 2, 2)? as i32;
    (hours <= 23 && minutes < 60).then_some(sign * (hours * 60 + minutes))
}

fn month_number(value: &str) -> Option<u32> {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .iter()
    .position(|month| month.eq_ignore_ascii_case(value) || month[..3].eq_ignore_ascii_case(value))
    .map(|index| index as u32 + 1)
}

fn format_datetime_value(date: DateTime, format: &str) -> String {
    if format.is_empty() {
        return format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            date.year, date.month, date.day, date.hour, date.minute, date.second
        );
    }
    let mut output = String::with_capacity(format.len() + 16);
    let mut chars = format.chars();
    while let Some(character) = chars.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        let Some(mut directive) = chars.next() else {
            output.push('%');
            break;
        };
        let mut pad = '0';
        if matches!(directive, '-' | '_' | '0') {
            pad = directive;
            let Some(next) = chars.next() else { break };
            directive = next;
        }
        let rendered = match directive {
            '%' => "%".into(),
            'Y' => format!("{:04}", date.year),
            'y' => format!("{:02}", date.year.rem_euclid(100)),
            'C' => format!("{:02}", date.year.div_euclid(100)),
            'm' => format!("{:02}", date.month),
            'd' => format!("{:02}", date.day),
            'e' => format!("{:2}", date.day),
            'H' => format!("{:02}", date.hour),
            'I' => format!("{:02}", hour12(date.hour)),
            'k' => format!("{:2}", date.hour),
            'l' => format!("{:2}", hour12(date.hour)),
            'M' => format!("{:02}", date.minute),
            'S' => format!("{:02}", date.second),
            'f' => format!("{:06}", date.microsecond),
            'j' => format!("{:03}", date.ordinal()),
            'U' => format!("{:02}", week_number(date, false)),
            'W' => format!("{:02}", week_number(date, true)),
            'V' => format!("{:02}", iso_week(date).0),
            'G' => format!("{:04}", iso_week(date).1),
            'g' => format!("{:02}", iso_week(date).1.rem_euclid(100)),
            'w' => date.weekday().to_string(),
            'u' => (if date.weekday() == 0 {
                7
            } else {
                date.weekday()
            })
            .to_string(),
            'a' => WEEKDAYS_SHORT[date.weekday() as usize].into(),
            'A' => WEEKDAYS_LONG[date.weekday() as usize].into(),
            'b' | 'h' => MONTHS_SHORT[date.month as usize - 1].into(),
            'B' => MONTHS_LONG[date.month as usize - 1].into(),
            'p' => if date.hour < 12 { "AM" } else { "PM" }.into(),
            'z' => date
                .offset_minutes
                .map(|offset| format!("{:+03}{:02}", offset / 60, offset.abs() % 60))
                .unwrap_or_default(),
            'Z' => if date.offset_minutes == Some(0) {
                "UTC"
            } else {
                ""
            }
            .into(),
            'F' => format!("{:04}-{:02}-{:02}", date.year, date.month, date.day),
            'T' => format!("{:02}:{:02}:{:02}", date.hour, date.minute, date.second),
            'R' => format!("{:02}:{:02}", date.hour, date.minute),
            'D' => format!(
                "{:02}/{:02}/{:02}",
                date.month,
                date.day,
                date.year.rem_euclid(100)
            ),
            'r' => format!(
                "{:02}:{:02}:{:02} {}",
                hour12(date.hour),
                date.minute,
                date.second,
                if date.hour < 12 { "AM" } else { "PM" }
            ),
            'x' => format!(
                "{:02}/{:02}/{:02}",
                date.month,
                date.day,
                date.year.rem_euclid(100)
            ),
            'X' => format!("{:02}:{:02}:{:02}", date.hour, date.minute, date.second),
            'n' => "\n".into(),
            't' => "\t".into(),
            's' => {
                let days = days_from_civil(date.year, date.month, date.day);
                (days * 86_400 + i64::from(date.hour * 3600 + date.minute * 60 + date.second))
                    .to_string()
            }
            'c' => format!(
                "{} {} {:2} {:02}:{:02}:{:02} {:04}",
                WEEKDAYS_SHORT[date.weekday() as usize],
                MONTHS_SHORT[date.month as usize - 1],
                date.day,
                date.hour,
                date.minute,
                date.second,
                date.year
            ),
            _ => format!("%{directive}"),
        };
        if rendered.len() == 2 && rendered.starts_with('0') {
            match pad {
                '_' => output.push(' '),
                '-' => {}
                _ => {
                    output.push_str(&rendered);
                    continue;
                }
            }
            output.push(rendered.chars().nth(1).unwrap_or_default());
        } else {
            output.push_str(&rendered);
        }
    }
    output
}

fn hour12(hour: u32) -> u32 {
    match hour % 12 {
        0 => 12,
        hour => hour,
    }
}

fn week_number(date: DateTime, monday_first: bool) -> u32 {
    let first_weekday = DateTime {
        month: 1,
        day: 1,
        ..date
    }
    .weekday();
    let week_start = u32::from(monday_first);
    let offset = (week_start + 7 - first_weekday) % 7;
    let day_index = date.ordinal() - 1;
    if day_index < offset {
        0
    } else {
        (day_index - offset) / 7 + 1
    }
}

fn iso_week(date: DateTime) -> (u32, i32) {
    let weekday = if date.weekday() == 0 {
        7
    } else {
        date.weekday()
    };
    let ordinal = date.ordinal() as i32;
    let week = (ordinal - weekday as i32 + 10) / 7;
    if week < 1 {
        let previous = date.year - 1;
        (weeks_in_iso_year(previous), previous)
    } else {
        let weeks = weeks_in_iso_year(date.year);
        if week as u32 > weeks {
            (1, date.year + 1)
        } else {
            (week as u32, date.year)
        }
    }
}

fn weeks_in_iso_year(year: i32) -> u32 {
    let jan1 = DateTime {
        year,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_minutes: None,
    }
    .weekday();
    if jan1 == 4 || (jan1 == 3 && is_leap_year(year)) {
        53
    } else {
        52
    }
}

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}

const WEEKDAYS_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const WEEKDAYS_LONG: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTHS_LONG: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

#[cfg(test)]
mod tests {
    use super::format_datetime;

    #[test]
    fn parses_and_formats_common_datetime_patterns() {
        assert_eq!(
            format_datetime("2024-02-29", "D%Y-%m-%d/%Y%m%d").as_deref(),
            Some("20240229")
        );
        assert_eq!(
            format_datetime("2024-02-29 13:04:05", "D%Y-%m-%d %H:%M:%S/%a %b %e %T").as_deref(),
            Some("Thu Feb 29 13:04:05")
        );
        assert_eq!(
            format_datetime("2024-02-29", "D%Y-%m-%d/%U-%W-%V-%G").as_deref(),
            Some("08-09-09-2024")
        );
        assert_eq!(
            format_datetime("2024-01-01", "D%Y-%m-%d/%U-%W").as_deref(),
            Some("00-01")
        );
        assert_eq!(
            format_datetime("2024-02-29 23:45", "D%Y-%m-%d %H:%M/O+2:30/%Y%m%d_%H%M").as_deref(),
            Some("20240301_0215")
        );
        assert_eq!(
            format_datetime("2024-02-29 23:45", "D%Y-%m-%d %H:%M/O-5:30/%Y%m%d_%H%M").as_deref(),
            Some("20240229_1815")
        );
        assert_eq!(
            format_datetime("2024-02-29 23:45", "D%Y-%m-%d %H:%M/O+0:30/%Y%m%d_%H%M").as_deref(),
            Some("20240229_2315")
        );
        assert_eq!(
            format_datetime("2024-August-01", "D%Y-%B-%d/%B").as_deref(),
            Some("August")
        );
        assert_eq!(
            format_datetime("2024-02-02", "D%Y-%m-%d/%_d|%-d|%d").as_deref(),
            Some(" 2|2|02")
        );
        assert_eq!(
            format_datetime("2024-02-30", "D%Y-%m-%d/%Y%m%d").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime(
                "2024-03-01T01:00:00+02:00",
                "D%Y-%m-%dT%H:%M:%S%z/%Y-%m-%d %H:%M"
            )
            .as_deref(),
            Some("2024-02-29 23:00")
        );
        assert_eq!(
            format_datetime("2024-02-29 13:04:05.987654", "D%Y-%m-%d %H:%M:%S.%f/%S.%f").as_deref(),
            Some("05.000000")
        );
    }
}
