//! Timestamps without a date library: logs are in UTC, the panel shows durations.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Days since 1970-01-01 to (year, month, day).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `2026-11-16 10:02:03` (UTC).
pub fn utc(secs: u64) -> String {
    let (y, m, d) = civil((secs / 86_400) as i64);
    let s = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

/// The current time for a log line, with milliseconds.
pub fn stamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}.{:03}Z", utc(now.as_secs()), now.subsec_millis())
}

/// The current time for a file name: `20261116-100203`.
pub fn file_stamp() -> String {
    utc(now_secs()).replace(['-', ':'], "").replace(' ', "-")
}

/// How long ago `since` (seconds since the epoch) was: `42s`, `5m`, `2h 3m`, `3d 4h`.
pub fn ago(since: u64) -> String {
    let s = now_secs().saturating_sub(since);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h {}m", s / 3600, s % 3600 / 60)
    } else {
        format!("{}d {}h", s / 86_400, s % 86_400 / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(utc(1_794_823_323), "2026-11-16 10:02:03");
    }
}
