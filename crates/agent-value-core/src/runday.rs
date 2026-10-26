//! Run days: natural days in Beijing time (UTC+8), the statistical unit of
//! the authenticity gate (v22 §9, §11.2). Beijing time has no DST.

use std::fmt;

use crate::constants::RUN_DAY_UTC_OFFSET_SECONDS;
use crate::error::{CoreError, Result};

const MS_PER_DAY: i64 = 86_400_000;

/// A civil date in Beijing time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunDay {
    year: i32,
    month: u8,
    day: u8,
}

impl RunDay {
    /// The Beijing-time date containing `unix_ms`.
    pub fn from_unix_ms(unix_ms: i64) -> Result<Self> {
        let local = unix_ms
            .checked_add(i64::from(RUN_DAY_UTC_OFFSET_SECONDS) * 1000)
            .ok_or(CoreError::Overflow { what: "run day" })?;
        let days = local.div_euclid(MS_PER_DAY);
        civil_from_days(days)
    }

    /// Parses `YYYY-MM-DD`.
    pub fn parse(s: &str) -> Result<Self> {
        let err = || CoreError::Parse {
            what: "run day",
            value: s.to_string(),
        };
        let mut it = s.split('-');
        let (Some(y), Some(m), Some(d), None) = (it.next(), it.next(), it.next(), it.next()) else {
            return Err(err());
        };
        if y.len() != 4 || m.len() != 2 || d.len() != 2 {
            return Err(err());
        }
        let year: i32 = y.parse().map_err(|_| err())?;
        let month: u8 = m.parse().map_err(|_| err())?;
        let day: u8 = d.parse().map_err(|_| err())?;
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return Err(err());
        }
        Ok(Self { year, month, day })
    }

    pub fn year(&self) -> i32 {
        self.year
    }
    pub fn month(&self) -> u8 {
        self.month
    }
    pub fn day(&self) -> u8 {
        self.day
    }

    /// UTC milliseconds of 00:00 on this Beijing civil date.
    ///
    /// Beijing is a fixed UTC+8 offset, so this is the UTC midnight of the
    /// same calendar date minus 8 hours. [`Self::from_unix_ms`] of this
    /// instant is this date; one millisecond earlier is the previous date.
    pub fn beijing_midnight_unix_ms(self) -> Result<i64> {
        let days = days_from_civil(self.year, u32::from(self.month), u32::from(self.day));
        let utc_midnight_ms = days.checked_mul(MS_PER_DAY).ok_or(CoreError::Overflow {
            what: "run day midnight",
        })?;
        utc_midnight_ms
            .checked_sub(i64::from(RUN_DAY_UTC_OFFSET_SECONDS) * 1000)
            .ok_or(CoreError::Overflow {
                what: "run day midnight",
            })
    }

    /// This date plus `days` in the proleptic Gregorian calendar.
    pub fn checked_add_days(self, days: i32) -> Result<Self> {
        let next = days_from_civil(self.year, u32::from(self.month), u32::from(self.day))
            .checked_add(i64::from(days))
            .ok_or(CoreError::Overflow { what: "run day" })?;
        civil_from_days(next)
    }
}

impl fmt::Display for RunDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i32, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Hinnant).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let mut y = i64::from(year);
    let m = i64::from(month);
    y -= i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    era * 146_097 + doe as i64 - 719_468
}

/// Days since 1970-01-01 to a proleptic Gregorian date (era-based method).
fn civil_from_days(z: i64) -> Result<RunDay> {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let year = i32::try_from(year).map_err(|_| CoreError::Overflow {
        what: "run day year",
    })?;
    Ok(RunDay {
        year,
        month: month as u8,
        day: day as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beijing_day_boundaries() {
        // 1970-01-01T00:00:00Z is 08:00 in Beijing.
        assert_eq!(RunDay::from_unix_ms(0).unwrap().to_string(), "1970-01-01");
        // 2026-10-19T15:59:59.999Z = 23:59:59.999 Beijing; one ms later is the next day.
        let t = 1_792_425_599_999;
        assert_eq!(RunDay::from_unix_ms(t).unwrap().to_string(), "2026-10-19");
        assert_eq!(
            RunDay::from_unix_ms(t + 1).unwrap().to_string(),
            "2026-10-20"
        );
        // Before the epoch: 1969-12-31T15:59:59Z = 23:59:59 Beijing on 12-31.
        assert_eq!(
            RunDay::from_unix_ms(-28_801_000).unwrap().to_string(),
            "1969-12-31"
        );
        // Leap day.
        assert_eq!(
            RunDay::from_unix_ms(1_709_164_800_000).unwrap().to_string(),
            "2024-02-29"
        );
    }

    #[test]
    fn parse_round_trip_and_errors() {
        let d = RunDay::parse("2024-02-29").unwrap();
        assert_eq!(d.to_string(), "2024-02-29");
        for bad in [
            "2023-02-29",
            "2024-13-01",
            "2024-1-01",
            "x",
            "2024-01-01-01",
            "2024-00-10",
        ] {
            assert!(RunDay::parse(bad).is_err(), "{bad}");
        }
        assert!(RunDay::from_unix_ms(i64::MAX).is_err());
    }

    #[test]
    fn beijing_midnight_matches_the_day_boundary() {
        let day = RunDay::parse("2026-10-20").unwrap();
        let midnight = day.beijing_midnight_unix_ms().unwrap();
        // 2026-10-19T16:00:00Z is 2026-10-20 00:00 in Beijing.
        assert_eq!(midnight, 1_792_425_600_000);
        assert_eq!(RunDay::from_unix_ms(midnight).unwrap(), day);
        assert_eq!(
            RunDay::from_unix_ms(midnight - 1).unwrap().to_string(),
            "2026-10-19"
        );
        assert_eq!(day.checked_add_days(1).unwrap().to_string(), "2026-10-21");
        assert_eq!(
            RunDay::parse("2026-10-31")
                .unwrap()
                .checked_add_days(1)
                .unwrap()
                .to_string(),
            "2026-11-01"
        );
        assert_eq!(
            RunDay::parse("2024-02-28")
                .unwrap()
                .checked_add_days(1)
                .unwrap()
                .to_string(),
            "2024-02-29"
        );
        let start = RunDay::parse("1969-12-31").unwrap();
        for offset in 0..800 {
            let day = start.checked_add_days(offset).unwrap();
            let z =
                super::days_from_civil(day.year(), u32::from(day.month()), u32::from(day.day()));
            assert_eq!(super::civil_from_days(z).unwrap(), day);
            let ms = day.beijing_midnight_unix_ms().unwrap();
            assert_eq!(RunDay::from_unix_ms(ms).unwrap(), day);
        }
    }
}
