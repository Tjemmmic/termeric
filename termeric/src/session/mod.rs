//! Terminal sessions: the per-terminal agent, transcript, and diff state
//! ([`terminal_state`]), transcripts and their export, and the workspace
//! panel and what it lists. Closed tabs are kept as one-tab workspaces
//! ([`crate::workspace`]).

pub mod export;
pub(crate) mod export_job;
pub mod listing;
pub mod panel;
pub(crate) mod terminal_state;
pub mod transcript;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
thread_local! {
    static TEST_SESSIONS_BASE_DIR: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

static SESSION_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Generate a sortable, collision-resistant session ID. Nanoseconds, process
/// ID, and a process-local atomic sequence avoid same-tick collisions.
pub fn generate_session_id() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let sequence = SESSION_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "{}_{}_{:010}_{:06}",
        format_unix_timestamp(duration.as_secs(), None).replace([':', 'T', 'Z'], ""),
        duration.subsec_nanos(),
        std::process::id(),
        sequence % 1_000_000,
    )
}

/// Format a non-negative Unix timestamp as UTC RFC 3339.
pub fn format_unix_timestamp(seconds: u64, millis: Option<u32>) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    match millis {
        Some(millis) => {
            format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
        }
        None => format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"),
    }
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Parse the UTC RFC 3339 form written by [`format_unix_timestamp`]
/// (`YYYY-MM-DDTHH:MM:SS`, optional fraction, `Z`) into Unix seconds.
pub fn parse_utc_timestamp(timestamp: &str) -> Option<u64> {
    let timestamp = timestamp.strip_suffix('Z')?;
    let timestamp = timestamp
        .split_once('.')
        .map_or(timestamp, |(whole, _)| whole);
    let (date, time) = timestamp.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut time = time.splitn(3, ':').map(str::parse::<u64>);
    let (hour, minute, second) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month as u32, day as u32);
    u64::try_from(days * 86_400)
        .ok()?
        .checked_add(hour * 3_600 + minute * 60 + second)
}

/// How long ago `timestamp` was, relative to `now` (Unix seconds), in the
/// words of [`crate::ui::age::prose_age`]; `None` when it can't be read.
pub fn describe_age(timestamp: &str, now: u64) -> Option<String> {
    Some(crate::ui::age::prose_age(
        parse_utc_timestamp(timestamp)?,
        now,
    ))
}

// Inverse of `civil_from_days`: days since 1970-01-01 for a civil date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

// Howard Hinnant's civil-from-days algorithm, with day zero at 1970-01-01.
pub(crate) fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

/// Where transcripts and session exports are written:
///
/// `~/.local/share/termeric/sessions/`.
pub fn sessions_base_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_SESSIONS_BASE_DIR.with(|base| base.borrow().clone()) {
        return Some(path);
    }
    crate::paths::data_dir().map(|dir| dir.join("sessions"))
}

#[cfg(test)]
pub(crate) fn set_test_sessions_base_dir(path: Option<PathBuf>) {
    TEST_SESSIONS_BASE_DIR.with(|base| *base.borrow_mut() = path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_format_handles_epoch_leap_day_and_milliseconds() {
        assert_eq!(format_unix_timestamp(0, None), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_unix_timestamp(1_582_934_400, Some(7)),
            "2020-02-29T00:00:00.007Z"
        );
    }

    #[test]
    fn generated_ids_are_unique_within_one_process() {
        let ids: std::collections::HashSet<_> =
            (0..10_000).map(|_| generate_session_id()).collect();
        assert_eq!(ids.len(), 10_000);
    }

    #[test]
    fn timestamps_round_trip_and_describe_their_age_plainly() {
        for seconds in [0, 86_399, 951_782_400, 1_790_200_000] {
            let stamp = format_unix_timestamp(seconds, Some(250));
            assert_eq!(parse_utc_timestamp(&stamp), Some(seconds), "{stamp}");
        }
        assert_eq!(parse_utc_timestamp("2026-09-23T19:50:12"), None);
        assert_eq!(parse_utc_timestamp("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_utc_timestamp("unreadable"), None);

        let now = parse_utc_timestamp("2026-09-23T12:00:00Z").unwrap();
        assert_eq!(
            describe_age("2026-09-23T09:00:00Z", now).as_deref(),
            Some("3 hours ago")
        );
        assert_eq!(describe_age("unreadable", now), None);
    }
}
