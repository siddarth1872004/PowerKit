//! Calendar arithmetic and the small `strftime` subset rename patterns support. No time
//! zone database is involved: callers convert to local time before formatting.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl DateTime {
    /// Seconds since 1970-01-01 (already shifted to the wanted time zone) to fields.
    pub fn from_unix(secs: i64) -> DateTime {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400) as u32;
        // Howard Hinnant's civil_from_days.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
        DateTime {
            year,
            month,
            day,
            hour: rem / 3_600,
            minute: rem / 60 % 60,
            second: rem % 60,
        }
    }

    /// Parse the EXIF form `YYYY:MM:DD HH:MM:SS`.
    pub fn parse_exif(s: &str) -> Option<DateTime> {
        let b = s.as_bytes();
        if b.len() < 19 || b[4] != b':' || b[7] != b':' || b[10] != b' ' || b[13] != b':' || b[16] != b':' {
            return None;
        }
        let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<u32>().ok();
        let dt = DateTime {
            year: num(0..4)? as i32,
            month: num(5..7)?,
            day: num(8..10)?,
            hour: num(11..13)?,
            minute: num(14..16)?,
            second: num(17..19)?,
        };
        let valid = (1..=12).contains(&dt.month)
            && (1..=31).contains(&dt.day)
            && dt.hour < 24
            && dt.minute < 60
            && dt.second < 61
            && dt.year > 0;
        valid.then_some(dt)
    }

    /// Format with `%Y %y %m %d %H %M %S %%`; any other `%x` is kept literally.
    pub fn format(&self, pattern: &str) -> String {
        let mut out = String::with_capacity(pattern.len() + 8);
        let mut chars = pattern.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('Y') => out.push_str(&format!("{:04}", self.year)),
                Some('y') => out.push_str(&format!("{:02}", self.year.rem_euclid(100))),
                Some('m') => out.push_str(&format!("{:02}", self.month)),
                Some('d') => out.push_str(&format!("{:02}", self.day)),
                Some('H') => out.push_str(&format!("{:02}", self.hour)),
                Some('M') => out.push_str(&format!("{:02}", self.minute)),
                Some('S') => out.push_str(&format!("{:02}", self.second)),
                Some('%') => out.push('%'),
                Some(other) => {
                    out.push('%');
                    out.push(other);
                }
                None => out.push('%'),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn known_dates() {
        let d = DateTime::from_unix(0);
        assert_eq!((d.year, d.month, d.day, d.hour), (1970, 1, 1, 0));
        // 2024-02-29 13:45:07 UTC
        let d = DateTime::from_unix(1_709_214_307);
        assert_eq!(d.format("%Y-%m-%d %H:%M:%S"), "2024-02-29 13:45:07");
        assert_eq!(d.format("%y%%%q"), "24%%q");
        let d = DateTime::from_unix(-1);
        assert_eq!(d.format("%Y-%m-%d %H:%M:%S"), "1969-12-31 23:59:59");
    }

    #[test]
    fn exif_parsing() {
        let d = DateTime::parse_exif("2021:07:04 09:08:07").unwrap();
        assert_eq!(d.format("%Y%m%d_%H%M%S"), "20210704_090807");
        assert_eq!(DateTime::parse_exif("0000:00:00 00:00:00"), None);
        assert_eq!(DateTime::parse_exif("2021-07-04 09:08:07"), None);
        assert_eq!(DateTime::parse_exif("2021:07"), None);
    }

    proptest! {
        #[test]
        fn fields_are_in_range(secs in -10_000_000_000i64..10_000_000_000) {
            let d = DateTime::from_unix(secs);
            prop_assert!((1..=12).contains(&d.month) && (1..=31).contains(&d.day));
            prop_assert!(d.hour < 24 && d.minute < 60 && d.second < 60);
            let next = DateTime::from_unix(secs + 86_400);
            prop_assert!((next.year, next.month, next.day) > (d.year, d.month, d.day));
        }

        #[test]
        fn exif_never_panics(s in ".{0,30}") {
            let _ = DateTime::parse_exif(&s);
        }
    }
}
