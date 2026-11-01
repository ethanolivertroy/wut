//! Today's date on this machine, for the system prompt.

unsafe extern "C" {
    // POSIX; the libc crate doesn't export it on every platform.
    fn tzset();
}

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// A sentence giving the local date, weekday and UTC offset, such as
/// `Today is Sunday, 2026-11-01 (local time, UTC-05:00).` Falls back to the
/// UTC date if the local time can't be read.
pub fn today() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    // SAFETY: tzset reads TZ and the zone files; localtime_r fills `tm`,
    // which is zeroed and owned here, and returns null on failure.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let local = unsafe {
        tzset();
        !libc::localtime_r(&now, &mut tm).is_null()
    };
    if !local {
        unsafe { libc::gmtime_r(&now, &mut tm) };
        return describe(&Fields::from(&tm), None);
    }
    describe(&Fields::from(&tm), Some(tm.tm_gmtoff as i64))
}

struct Fields {
    year: i32,
    month: i32,
    day: i32,
    weekday: usize,
}

impl From<&libc::tm> for Fields {
    fn from(tm: &libc::tm) -> Self {
        Self {
            year: tm.tm_year + 1900,
            month: tm.tm_mon + 1,
            day: tm.tm_mday,
            weekday: usize::try_from(tm.tm_wday).unwrap_or(0) % 7,
        }
    }
}

fn describe(date: &Fields, offset_seconds: Option<i64>) -> String {
    let when = format!(
        "{}, {:04}-{:02}-{:02}",
        WEEKDAYS[date.weekday], date.year, date.month, date.day
    );
    match offset_seconds {
        Some(offset) => {
            let sign = if offset < 0 { '-' } else { '+' };
            let minutes = offset.unsigned_abs() / 60;
            format!(
                "Today is {when} (local time, UTC{sign}{:02}:{:02}).",
                minutes / 60,
                minutes % 60
            )
        }
        None => format!("Today is {when} (UTC)."),
    }
}

#[cfg(test)]
mod tests {
    use super::{Fields, describe, today};

    fn date(year: i32, month: i32, day: i32, weekday: usize) -> Fields {
        Fields {
            year,
            month,
            day,
            weekday,
        }
    }

    #[test]
    fn describes_the_date_and_offset() {
        assert_eq!(
            describe(&date(2026, 11, 1, 0), Some(-5 * 3600)),
            "Today is Sunday, 2026-11-01 (local time, UTC-05:00)."
        );
        assert_eq!(
            describe(&date(2026, 3, 9, 1), Some(5 * 3600 + 1800)),
            "Today is Monday, 2026-03-09 (local time, UTC+05:30)."
        );
        assert_eq!(
            describe(&date(2026, 12, 31, 4), None),
            "Today is Thursday, 2026-12-31 (UTC)."
        );
    }

    #[test]
    fn reads_the_clock() {
        let today = today();
        assert!(today.starts_with("Today is "), "{today}");
        assert!(today.contains("-"), "{today}");
    }
}
