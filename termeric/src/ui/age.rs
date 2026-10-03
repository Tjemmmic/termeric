//! How long ago something happened, in a compact column or in prose.

use std::time::Duration;

/// For tight columns: "now", "2m", "3h", "4d".
pub fn compact_age(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    match seconds {
        0..60 => "now".to_owned(),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// For lists that read as words: "Just now", "5 minutes ago", "Yesterday",
/// "3 days ago", then the date ("Sep 12", with the year once it differs).
/// Times are Unix seconds; a future `then` counts as now. Dates are UTC, so
/// one near midnight may show the adjacent local day.
pub fn prose_age(then: u64, now: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let elapsed = now.saturating_sub(then);
    let plural = |count: u64, unit: &str| {
        if count == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{count} {unit}s ago")
        }
    };
    match elapsed {
        0..60 => "Just now".into(),
        60..3_600 => plural(elapsed / 60, "minute"),
        3_600..86_400 => plural(elapsed / 3_600, "hour"),
        86_400..172_800 => "Yesterday".into(),
        172_800..604_800 => plural(elapsed / 86_400, "day"),
        _ => {
            let (year, month, day) = crate::session::civil_from_days((then / 86_400) as i64);
            let (current_year, _, _) = crate::session::civil_from_days((now / 86_400) as i64);
            let month = MONTHS[month as usize - 1];
            if year == current_year {
                format!("{month} {day}")
            } else {
                format!("{month} {day}, {year}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_ages_fit_a_column() {
        for (seconds, expected) in [(5, "now"), (300, "5m"), (7_200, "2h"), (300_000, "3d")] {
            assert_eq!(compact_age(Duration::from_secs(seconds)), expected);
        }
    }

    #[test]
    fn prose_ages_read_naturally() {
        let now = crate::session::parse_utc_timestamp("2026-09-23T12:00:00Z").unwrap();
        let age = |stamp: &str| prose_age(crate::session::parse_utc_timestamp(stamp).unwrap(), now);
        assert_eq!(age("2026-09-23T11:59:30Z"), "Just now");
        assert_eq!(age("2026-09-23T11:59:00Z"), "1 minute ago");
        assert_eq!(age("2026-09-23T11:15:00Z"), "45 minutes ago");
        assert_eq!(age("2026-09-23T09:00:00Z"), "3 hours ago");
        assert_eq!(age("2026-09-22T10:00:00Z"), "Yesterday");
        assert_eq!(age("2026-09-19T12:00:00Z"), "4 days ago");
        assert_eq!(age("2026-09-02T12:00:00Z"), "Sep 2");
        assert_eq!(age("2025-12-31T23:00:00Z"), "Dec 31, 2025");
        assert_eq!(
            age("2026-09-24T00:00:00Z"),
            "Just now",
            "clock skew is not negative"
        );
    }
}
