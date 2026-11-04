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
    offset_seconds: Option<i32>,
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
        if let Some(offset) = self.offset_seconds.take() {
            let seconds = days_from_civil(self.year, self.month, self.day) * 86_400
                + i64::from(self.hour) * 3600
                + i64::from(self.minute) * 60
                + i64::from(self.second)
                - i64::from(offset);
            let days = seconds.div_euclid(86_400);
            let day_seconds = seconds.rem_euclid(86_400);
            (self.year, self.month, self.day) = civil_from_days(days);
            self.hour = (day_seconds / 3600) as u32;
            self.minute = (day_seconds % 3600 / 60) as u32;
            self.second = (day_seconds % 60) as u32;
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

/// Convert gallery-dl's `d` and `D` filename conversions to a UTC datetime
/// string, optionally applying Python-style datetime format directives.
pub(crate) fn convert_datetime(
    value: &str,
    source: Option<&serde_json::Value>,
    spec: &str,
    parse_iso: bool,
) -> String {
    use serde_json::Value;

    let date = match source {
        Some(Value::Number(number)) => number
            .as_f64()
            .filter(|seconds| !parse_iso || *seconds != 0.0)
            .and_then(datetime_from_timestamp),
        Some(Value::Bool(boolean)) if parse_iso && !boolean => None,
        Some(Value::Bool(boolean)) => datetime_from_timestamp(f64::from(u8::from(*boolean))),
        Some(Value::String(value)) => value
            .parse::<i64>()
            .ok()
            .and_then(|seconds| datetime_from_timestamp(seconds as f64))
            .or_else(|| parse_iso.then(|| parse_iso_datetime(value)).flatten()),
        _ => value
            .parse::<i64>()
            .ok()
            .and_then(|seconds| datetime_from_timestamp(seconds as f64))
            .or_else(|| parse_iso.then(|| parse_iso_datetime(value)).flatten()),
    };
    date.filter(|date| date.valid())
        .map(|date| format_datetime_value(date, spec))
        .unwrap_or_else(|| "[Invalid DateTime]".into())
}

fn datetime_from_timestamp(seconds: f64) -> Option<DateTime> {
    let seconds = seconds.floor();
    if !(i64::MIN as f64..=i64::MAX as f64).contains(&seconds) {
        return None;
    }
    let seconds = seconds as i64;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let date = DateTime {
        year,
        month,
        day,
        hour: (day_seconds / 3600) as u32,
        minute: (day_seconds % 3600 / 60) as u32,
        second: (day_seconds % 60) as u32,
        microsecond: 0,
        offset_seconds: None,
    };
    date.valid().then_some(date)
}

fn parse_iso_datetime(value: &str) -> Option<DateTime> {
    let mut value = value
        .strip_suffix('Z')
        .map_or_else(|| value.to_owned(), |prefix| format!("{prefix}+00:00"));
    if value.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        if let Some(separator) = value.get(10..).and_then(|tail| tail.chars().next()) {
            if separator != 'T' {
                let suffix_start = 10 + separator.len_utf8();
                value = format!("{}T{}", &value[..10], &value[suffix_start..]);
            }
        }
    }
    if let Some(offset_start) = value
        .char_indices()
        .filter(|(index, character)| *index > 10 && matches!(character, '+' | '-'))
        .map(|(index, _)| index)
        .next_back()
    {
        if value.len() - offset_start == 3 {
            value.push_str(":00");
        }
    }
    if let Some(comma) = value
        .char_indices()
        .find(|(index, character)| *index > 10 && *character == ',')
        .map(|(index, _)| index)
    {
        value.replace_range(comma..comma + 1, ".");
    }
    [
        "%Y-%m-%dT%H:%M:%S.%f%z",
        "%Y-%m-%dT%H:%M:%S%z",
        "%Y-%m-%dT%H:%M%z",
        "%Y-%m-%dT%H:%M:%S.%f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S.%f%z",
        "%Y-%m-%d %H:%M:%S%z",
        "%Y-%m-%d %H:%M%z",
        "%Y-%m-%d %H:%M:%S.%f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d",
    ]
    .into_iter()
    .find_map(|format| parse_datetime(&value, format))
    .map(DateTime::normalize_offset)
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
        offset_seconds: None,
    };
    let mut twelve_hour_clock = false;
    let mut ordinal_day = None;
    let mut sunday_week = None;
    let mut monday_week = None;
    let mut iso_year = None;
    let mut iso_week = None;
    let mut weekday_number = None;
    let mut iso_weekday_number = None;
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
            'Y' => date.year = read_number(value, &mut input, 4, 4)? as i32,
            'G' => iso_year = Some(read_number(value, &mut input, 4, 4)? as i32),
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
                if !(1..=12).contains(&hour) {
                    return None;
                }
                date.hour = hour % 12;
                twelve_hour_clock = true;
            }
            'M' => date.minute = read_number(value, &mut input, 1, 2)?,
            'S' => date.second = read_number(value, &mut input, 1, 2)?,
            'f' => {
                let start = input;
                let micros = read_number(value, &mut input, 1, 6)?;
                date.microsecond = micros * 10u32.pow(6 - (input - start) as u32);
            }
            'j' => {
                ordinal_day = Some(read_number(value, &mut input, 1, 3)?);
            }
            'U' => {
                sunday_week = Some(read_number(value, &mut input, 1, 2)?);
            }
            'W' => {
                monday_week = Some(read_number(value, &mut input, 1, 2)?);
            }
            'V' => {
                iso_week = Some(read_number(value, &mut input, 1, 2)?);
            }
            'w' => {
                let weekday = read_number(value, &mut input, 1, 1)?;
                if weekday > 6 {
                    return None;
                }
                weekday_number = Some(weekday);
                iso_weekday_number = Some(if weekday == 0 { 7 } else { weekday });
            }
            'u' => {
                let weekday = read_number(value, &mut input, 1, 1)?;
                if !(1..=7).contains(&weekday) {
                    return None;
                }
                weekday_number = Some(weekday % 7);
                iso_weekday_number = Some(weekday);
            }
            'z' => date.offset_seconds = Some(read_offset(value, &mut input)?),
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
                    'b' => date.month = month_number(word, false)?,
                    'B' => date.month = month_number(word, true)?,
                    'a' if WEEKDAYS_SHORT
                        .iter()
                        .any(|weekday| weekday.eq_ignore_ascii_case(word)) => {}
                    'A' if WEEKDAYS_LONG
                        .iter()
                        .any(|weekday| weekday.eq_ignore_ascii_case(word)) => {}
                    'p' if word.eq_ignore_ascii_case("PM") && twelve_hour_clock => {
                        date.hour = (date.hour % 12) + 12;
                    }
                    'p' if word.eq_ignore_ascii_case("AM") && twelve_hour_clock => {
                        date.hour %= 12;
                    }
                    'p' if word.eq_ignore_ascii_case("AM") || word.eq_ignore_ascii_case("PM") => {}
                    'p' => return None,
                    'a' | 'A' => return None,
                    'Z' if word.eq_ignore_ascii_case("UTC") || word.eq_ignore_ascii_case("GMT") => {
                    }
                    'Z' => return None,
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
    if let Some(ordinal) = ordinal_day {
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
    } else if let (Some(year), Some(week), Some(weekday)) = (iso_year, iso_week, iso_weekday_number)
    {
        if !(1..=9999).contains(&year) || !(1..=53).contains(&week) || weekday == 0 {
            return None;
        }
        apply_iso_week_date(&mut date, year, week, weekday)?;
    } else if let (Some(week), Some(weekday)) = (sunday_week, weekday_number) {
        if week > 53 {
            return None;
        }
        apply_week_date(&mut date, week, weekday, false);
    } else if let (Some(week), Some(weekday)) = (monday_week, weekday_number) {
        if week > 53 {
            return None;
        }
        apply_week_date(&mut date, week, weekday, true);
    }
    (input == value.len() && date.valid()).then_some(date)
}

fn apply_iso_week_date(date: &mut DateTime, year: i32, week: u32, weekday: u32) -> Option<()> {
    if week > weeks_in_iso_year(year) {
        return None;
    }
    let january_fourth = days_from_civil(year, 1, 4);
    let sunday_weekday = (january_fourth + 4).rem_euclid(7) as u32;
    let monday = january_fourth - i64::from((sunday_weekday + 6) % 7);
    let days = monday + i64::from((week - 1) * 7 + weekday - 1);
    (date.year, date.month, date.day) = civil_from_days(days);
    (iso_week(*date) == (week, year)).then_some(())
}

fn apply_week_date(date: &mut DateTime, week: u32, weekday: u32, monday_first: bool) {
    let year_start = days_from_civil(date.year, 1, 1);
    let first_sunday_weekday = (year_start + 4).rem_euclid(7) as u32;
    let first_weekday = if monday_first {
        (first_sunday_weekday + 6) % 7
    } else {
        first_sunday_weekday
    };
    let week_start = u32::from(monday_first);
    let offset = (week_start + 7 - first_sunday_weekday) % 7;
    let weekday = if monday_first {
        (weekday + 6) % 7
    } else {
        weekday
    };
    let day_of_year = if week == 0 {
        i64::from(weekday) - i64::from(first_weekday)
    } else {
        i64::from(offset + (week - 1) * 7 + weekday)
    };
    (date.year, date.month, date.day) = civil_from_days(year_start + day_of_year);
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
    let mut seconds = 0;
    if value.as_bytes().get(*cursor) == Some(&b':') {
        *cursor += 1;
        seconds = read_number(value, cursor, 2, 2)? as i32;
    }
    (hours <= 23 && minutes < 60 && seconds < 60)
        .then_some(sign * (hours * 3600 + minutes * 60 + seconds))
}

fn month_number(value: &str, full_name: bool) -> Option<u32> {
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
    .position(|month| {
        if full_name {
            month.eq_ignore_ascii_case(value)
        } else {
            month[..3].eq_ignore_ascii_case(value)
        }
    })
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
            'Y' => date.year.to_string(),
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
                .offset_seconds
                .map(|offset| format!("{:+03}{:02}", offset / 3600, offset.abs() / 60 % 60))
                .unwrap_or_default(),
            'Z' => if date.offset_seconds == Some(0) {
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
        offset_seconds: None,
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
    use super::{convert_datetime, format_datetime};

    #[test]
    fn converts_unix_and_iso_datetime_filename_values() {
        assert_eq!(
            convert_datetime("1709211845", None, "", false),
            "2024-02-29 13:04:05"
        );
        assert_eq!(
            convert_datetime("2024-02-29T13:04:05.987654+02:00", None, "", true),
            "2024-02-29 11:04:05"
        );
        assert_eq!(
            convert_datetime("2024-02-29t13:04:05+02:00", None, "%Y-%m-%d_%H-%M-%S", true),
            "2024-02-29_11-04-05"
        );
        assert_eq!(
            convert_datetime("2024-02-29 13:04:05+02", None, "%Y-%m-%d_%H-%M-%S", true),
            "2024-02-29_11-04-05"
        );
        assert_eq!(
            convert_datetime(
                "2024-02-29_13:04:05+02:30:15",
                None,
                "%Y-%m-%d_%H-%M-%S",
                true,
            ),
            "2024-02-29_10-33-50"
        );
        assert_eq!(
            convert_datetime("2024-02-29T13:04:05,5Z", None, "%Y-%m-%d_%H-%M-%S_%f", true,),
            "2024-02-29_13-04-05_000000"
        );
        assert_eq!(
            convert_datetime("2024-02-29T13:04:05Z", None, "%Y%m%d_%H%M%S_%f%z", true,),
            "20240229_130405_000000"
        );
        assert_eq!(
            convert_datetime("not-a-date", None, "%Y", true),
            "[Invalid DateTime]"
        );
        assert_eq!(
            convert_datetime("not-a-timestamp", None, "", false),
            "[Invalid DateTime]"
        );
    }

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
            format_datetime("2024-01-01 UTC", "D%Y-%m-%d %Z/%Y").as_deref(),
            Some("2024")
        );
        assert_eq!(
            format_datetime("2024-01-01 XYZ", "D%Y-%m-%d %Z/%Y").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("PM 2024", "D%p %Y/%H").as_deref(),
            Some("00")
        );
        assert_eq!(format_datetime("11 PM", "D%I %p/%H").as_deref(), Some("23"));
        assert_eq!(
            format_datetime("13 PM", "D%I %p/%H").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("Thursday 2024-02-29", "D%A %Y-%m-%d/%Y").as_deref(),
            Some("2024")
        );
        assert_eq!(
            format_datetime("Funday 2024-02-29", "D%A %Y-%m-%d/%Y").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("Thursday 2024-02-29", "D%a %Y-%m-%d/%Y").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("January 2024", "D%b %Y/%m").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("Jan 2024", "D%B %Y/%m").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("2024-02-29 4", "D%Y-%m-%d %w/%Y%m%d").as_deref(),
            Some("20240229")
        );
        assert_eq!(
            format_datetime("2024-02-29 4", "D%Y-%m-%d %u/%Y%m%d").as_deref(),
            Some("20240229")
        );
        assert_eq!(
            format_datetime("2024-02-29 8", "D%Y-%m-%d %u/%Y%m%d").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("2024 09 4", "D%Y %U %w/%Y-%m-%d").as_deref(),
            Some("2024-03-07")
        );
        assert_eq!(
            format_datetime("2024 09 4", "D%Y %W %w/%Y-%m-%d").as_deref(),
            Some("2024-02-29")
        );
        assert_eq!(
            format_datetime("2024 00 0", "D%Y %U %w/%Y-%m-%d").as_deref(),
            Some("2023-12-31")
        );
        assert_eq!(
            format_datetime("2024 060 09 4", "D%Y %j %U %w/%Y-%m-%d").as_deref(),
            Some("2024-02-29")
        );
        assert_eq!(
            format_datetime("2020-W01-1", "D%G-W%V-%u/%Y-%m-%d").as_deref(),
            Some("2019-12-30")
        );
        assert_eq!(
            format_datetime("2020-W53-7", "D%G-W%V-%u/%Y-%m-%d").as_deref(),
            Some("2021-01-03")
        );
        assert_eq!(
            format_datetime("2021-W53-1", "D%G-W%V-%u/%Y-%m-%d").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("0007-01-01", "D%Y-%m-%d/%Y").as_deref(),
            Some("7")
        );
        assert_eq!(
            format_datetime("7-01-01", "D%Y-%m-%d/%Y").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("13:04", "D%R/%H:%M").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("13:04:05", "D%T/%H:%M:%S").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("01:04:05 PM", "D%r/%H:%M:%S").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("13:04:05 PM", "D%r/%H").as_deref(),
            Some("[Invalid DateTime]")
        );
        assert_eq!(
            format_datetime("366 2024", "D%j %Y/%Y%m%d").as_deref(),
            Some("20241231")
        );
        assert_eq!(
            format_datetime("2024 366", "D%Y %j/%Y%m%d").as_deref(),
            Some("20241231")
        );
        assert_eq!(
            format_datetime("2024 060 01-01", "D%Y %j %m-%d/%Y%m%d").as_deref(),
            Some("20240229")
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
            format_datetime(
                "2024-03-01T01:00:00+02:30:15",
                "D%Y-%m-%dT%H:%M:%S%z/%Y-%m-%d %H:%M:%S"
            )
            .as_deref(),
            Some("2024-02-29 22:29:45")
        );
        assert_eq!(
            format_datetime("2024-02-29 13:04:05.987654", "D%Y-%m-%d %H:%M:%S.%f/%S.%f").as_deref(),
            Some("05.000000")
        );
    }
}
