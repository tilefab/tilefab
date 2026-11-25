//! Wall-clock time of the date (`D`) and time (`T`) components.

use std::fmt::{self, Write};

/// A date and a time of day, already in local time, given by the caller to
/// [`LineWidget::set_time`](crate::LineWidget::set_time).
///
/// `tilefab-widget` has no date or time-zone dependency: the application converts its clock
/// (for instance with the `jiff` crate) and fills the fields. [`WallTime::default()`] has every
/// field at 0: it is what the widget shows before the first `set_time`
/// (`0000-00-00 00:00:00.000`), but it is not a valid argument of `set_time`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WallTime {
    /// Year, 0–9999.
    pub year: u16,
    /// Month, 1–12.
    pub month: u8,
    /// Day of the month, 1–28, 29, 30 or 31 depending on the month and the year.
    pub day: u8,
    /// Hour, 0–23.
    pub hour: u8,
    /// Minute, 0–59.
    pub minute: u8,
    /// Second, 0–59.
    pub second: u8,
    /// Millisecond, 0–999.
    pub millis: u16,
}

/// The field of a [`WallTime`] that is out of its range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TimeField {
    /// [`WallTime::year`].
    Year,
    /// [`WallTime::month`].
    Month,
    /// [`WallTime::day`].
    Day,
    /// [`WallTime::hour`].
    Hour,
    /// [`WallTime::minute`].
    Minute,
    /// [`WallTime::second`].
    Second,
    /// [`WallTime::millis`].
    Millis,
}

impl fmt::Display for TimeField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Year => "year",
            Self::Month => "month",
            Self::Day => "day",
            Self::Hour => "hour",
            Self::Minute => "minute",
            Self::Second => "second",
            Self::Millis => "millis",
        })
    }
}

impl WallTime {
    /// Check every field: the first one out of its range is returned as an error.
    ///
    /// # Errors
    ///
    /// The [`TimeField`] out of its range.
    pub fn validate(&self) -> Result<(), TimeField> {
        let checks = [
            (self.year <= 9999, TimeField::Year),
            ((1..=12).contains(&self.month), TimeField::Month),
            (
                self.day >= 1 && self.day <= days_in_month(self.year, self.month),
                TimeField::Day,
            ),
            (self.hour <= 23, TimeField::Hour),
            (self.minute <= 59, TimeField::Minute),
            (self.second <= 59, TimeField::Second),
            (self.millis <= 999, TimeField::Millis),
        ];
        match checks.into_iter().find(|&(ok, _)| !ok) {
            Some((_, field)) => Err(field),
            None => Ok(()),
        }
    }
}

/// Number of days of `month` (1–12) in `year`, in the proleptic Gregorian calendar; 0 for a
/// month out of range.
fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Leap year: divisible by 4, except the centuries not divisible by 400.
fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// Width in columns of the date format `format` (0–2).
pub(crate) fn date_width(format: u8) -> usize {
    [10, 8, 5][usize::from(format)]
}

/// Width in columns of the time format `format` (0–5).
pub(crate) fn time_width(format: u8) -> usize {
    [12, 9, 6, 8, 5, 5][usize::from(format)]
}

/// Write the date of `time` in format `format`: 0 `YYYY-MM-DD`, 1 `YY-MM-DD`, 2 `MM-DD`.
pub(crate) fn write_date(out: &mut impl Write, time: &WallTime, format: u8) -> fmt::Result {
    let WallTime {
        year, month, day, ..
    } = *time;
    match format {
        0 => write!(out, "{year:04}-{month:02}-{day:02}"),
        1 => write!(out, "{:02}-{month:02}-{day:02}", year % 100),
        _ => write!(out, "{month:02}-{day:02}"),
    }
}

/// Write the time of `time` in format `format`: 0 `HH:MM:SS.SSS`, 1 `MM:SS.SSS`,
/// 2 `SS.SSS`, 3 `HH:MM:SS`, 4 `HH:MM`, 5 `MM:SS`.
pub(crate) fn write_time(out: &mut impl Write, time: &WallTime, format: u8) -> fmt::Result {
    let WallTime {
        hour,
        minute: min,
        second: sec,
        millis: ms,
        ..
    } = *time;
    match format {
        0 => write!(out, "{hour:02}:{min:02}:{sec:02}.{ms:03}"),
        1 => write!(out, "{min:02}:{sec:02}.{ms:03}"),
        2 => write!(out, "{sec:02}.{ms:03}"),
        3 => write!(out, "{hour:02}:{min:02}:{sec:02}"),
        4 => write!(out, "{hour:02}:{min:02}"),
        _ => write!(out, "{min:02}:{sec:02}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIME: WallTime = WallTime {
        year: 2026,
        month: 3,
        day: 7,
        hour: 9,
        minute: 5,
        second: 4,
        millis: 23,
    };

    fn date(time: &WallTime, format: u8) -> String {
        let mut out = String::new();
        write_date(&mut out, time, format).unwrap();
        out
    }

    fn clock(time: &WallTime, format: u8) -> String {
        let mut out = String::new();
        write_time(&mut out, time, format).unwrap();
        out
    }

    #[test]
    fn the_nine_formats() {
        let dates = ["2026-03-07", "26-03-07", "03-07"];
        for (format, expected) in (0..).zip(dates) {
            assert_eq!(date(&TIME, format), expected);
            assert_eq!(expected.len(), date_width(format));
        }
        let times = [
            "09:05:04.023",
            "05:04.023",
            "04.023",
            "09:05:04",
            "09:05",
            "05:04",
        ];
        for (format, expected) in (0..).zip(times) {
            assert_eq!(clock(&TIME, format), expected);
            assert_eq!(expected.len(), time_width(format));
        }
    }

    #[test]
    fn zeros_before_the_first_set_time() {
        let zero = WallTime::default();
        assert_eq!(date(&zero, 0), "0000-00-00");
        assert_eq!(clock(&zero, 0), "00:00:00.000");
    }

    #[test]
    fn year_two_digits_and_four_digits() {
        let early = WallTime { year: 5, ..TIME };
        assert_eq!(date(&early, 0), "0005-03-07");
        assert_eq!(date(&early, 1), "05-03-07");
        let late = WallTime { year: 9999, ..TIME };
        assert_eq!(date(&late, 1), "99-03-07");
    }

    #[test]
    fn ranges() {
        assert_eq!(TIME.validate(), Ok(()));
        let cases = [
            (
                WallTime {
                    year: 10000,
                    ..TIME
                },
                TimeField::Year,
            ),
            (WallTime { month: 0, ..TIME }, TimeField::Month),
            (WallTime { month: 13, ..TIME }, TimeField::Month),
            (WallTime { day: 0, ..TIME }, TimeField::Day),
            (WallTime { day: 32, ..TIME }, TimeField::Day),
            (
                WallTime {
                    month: 4,
                    day: 31,
                    ..TIME
                },
                TimeField::Day,
            ),
            (WallTime { hour: 24, ..TIME }, TimeField::Hour),
            (WallTime { minute: 60, ..TIME }, TimeField::Minute),
            (WallTime { second: 60, ..TIME }, TimeField::Second),
            (
                WallTime {
                    millis: 1000,
                    ..TIME
                },
                TimeField::Millis,
            ),
        ];
        for (time, field) in cases {
            assert_eq!(time.validate(), Err(field), "{time:?}");
        }
        assert_eq!(WallTime::default().validate(), Err(TimeField::Month));
        let edges = [
            WallTime { year: 0, ..TIME },
            WallTime { year: 9999, ..TIME },
            WallTime {
                hour: 23,
                minute: 59,
                second: 59,
                millis: 999,
                ..TIME
            },
            WallTime {
                hour: 0,
                minute: 0,
                second: 0,
                millis: 0,
                ..TIME
            },
            WallTime {
                month: 12,
                day: 31,
                ..TIME
            },
        ];
        for time in edges {
            assert_eq!(time.validate(), Ok(()), "{time:?}");
        }
    }

    #[test]
    fn february_29() {
        let feb = |year, day| WallTime {
            year,
            month: 2,
            day,
            ..TIME
        };
        assert_eq!(feb(2024, 29).validate(), Ok(()));
        assert_eq!(feb(2000, 29).validate(), Ok(()));
        assert_eq!(feb(0, 29).validate(), Ok(()));
        assert_eq!(feb(2026, 29).validate(), Err(TimeField::Day));
        assert_eq!(feb(1900, 29).validate(), Err(TimeField::Day));
        assert_eq!(feb(2026, 28).validate(), Ok(()));
        assert_eq!(feb(2024, 30).validate(), Err(TimeField::Day));
    }
}
