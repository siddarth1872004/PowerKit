//! Sunrise and sunset from the NOAA solar equations (accurate to a few minutes, which
//! is plenty for switching a theme).

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SunDay {
    /// Local minutes after midnight, already wrapped into `0..1440`.
    Times { rise: u32, set: u32 },
    /// The sun does not set.
    PolarDay,
    /// The sun does not rise.
    PolarNight,
}

/// Day of the year (1-366) of the day `days` after 1970-01-01.
pub fn day_of_year(days: i64) -> u32 {
    // Civil-from-days (H. Hinnant), with years starting on 1 March so the leap day is last.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let from_march = doe - (365 * yoe + yoe / 4 - yoe / 100);
    if from_march >= 306 {
        return (from_march - 306 + 1) as u32;
    }
    let year = yoe + era * 400;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    (from_march + 59 + i64::from(leap) + 1) as u32
}

/// Sunrise and sunset on `day_of_year` at the given position, for a clock that is
/// `utc_offset_min` minutes ahead of UTC.
pub fn sun_day(day_of_year: u32, latitude: f64, longitude: f64, utc_offset_min: i32) -> SunDay {
    let g = 2.0 * std::f64::consts::PI / 365.0 * (f64::from(day_of_year) - 0.5);
    let eqtime = 229.18
        * (0.000075 + 0.001868 * g.cos()
            - 0.032077 * g.sin()
            - 0.014615 * (2.0 * g).cos()
            - 0.040849 * (2.0 * g).sin());
    let decl = 0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos()
        + 0.000907 * (2.0 * g).sin()
        - 0.002697 * (3.0 * g).cos()
        + 0.00148 * (3.0 * g).sin();
    let lat = latitude.to_radians();
    // 90.833 degrees: the sun's apparent radius plus atmospheric refraction.
    let cos_ha = 90.833f64.to_radians().cos() / (lat.cos() * decl.cos()) - lat.tan() * decl.tan();
    if cos_ha.is_nan() || cos_ha > 1.0 {
        return SunDay::PolarNight;
    }
    if cos_ha < -1.0 {
        return SunDay::PolarDay;
    }
    let ha = cos_ha.acos().to_degrees();
    let local = |utc: f64| (utc.round() as i64 + i64::from(utc_offset_min)).rem_euclid(1440) as u32;
    SunDay::Times {
        rise: local(720.0 - 4.0 * (longitude + ha) - eqtime),
        set: local(720.0 - 4.0 * (longitude - ha) - eqtime),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(actual: u32, expected: u32) -> bool {
        actual.abs_diff(expected) <= 5
    }

    #[test]
    fn day_numbers() {
        assert_eq!(day_of_year(0), 1);
        assert_eq!(day_of_year(-1), 365);
        assert_eq!(day_of_year(19_417), 60); // 2023-03-01
        assert_eq!(day_of_year(19_783), 61); // 2024-03-01
        assert_eq!(day_of_year(20_088), 366); // 2024-12-31
        assert_eq!(day_of_year(20_625), 172); // 2026-06-21
    }

    #[test]
    fn london_midsummer_in_summer_time() {
        let SunDay::Times { rise, set } = sun_day(172, 51.5, -0.13, 60) else {
            panic!("London has a sunrise in June");
        };
        assert!(near(rise, 4 * 60 + 43), "{rise}");
        assert!(near(set, 21 * 60 + 21), "{set}");
    }

    #[test]
    fn equinox_on_the_equator_and_wrapping() {
        let SunDay::Times { rise, set } = sun_day(79, 0.0, 0.0, 0) else {
            panic!()
        };
        assert!(near(rise, 6 * 60 + 4), "{rise}");
        assert!(near(set, 18 * 60 + 11), "{set}");
        // Far west with a UTC clock: sunset falls after midnight.
        let SunDay::Times { set, .. } = sun_day(79, 0.0, -120.0, 0) else {
            panic!()
        };
        assert!(near(set, 2 * 60 + 11), "{set}");
    }

    #[test]
    fn polar_day_and_night() {
        assert_eq!(sun_day(172, 69.6, 18.9, 120), SunDay::PolarDay);
        assert_eq!(sun_day(355, 69.6, 18.9, 60), SunDay::PolarNight);
        assert_eq!(sun_day(355, -69.6, 18.9, 60), SunDay::PolarDay);
        assert_eq!(sun_day(172, 90.0, 0.0, 0), SunDay::PolarDay);
    }
}
