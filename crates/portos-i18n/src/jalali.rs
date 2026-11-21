//! Persian (Jalali / Solar Hijri) calendar conversion (PRD L-3).
//!
//! Implements the well-known algorithm behind `jalaali-js` / Borkowski's
//! work (33-year cycle approximation of the true solar calendar), with
//! truncating integer division semantics exactly as in the reference
//! implementations. Storage and APIs remain Gregorian ISO 8601; conversion
//! is display-only.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

/// Valid range guard (algorithm domain).
const MIN_JALALI_YEAR: i32 = -60;
const MAX_JALALI_YEAR: i32 = 3177;

const BREAKS: [i32; 20] = [
    -61, 9, 38, 199, 426, 686, 756, 818, 1111, 1181, 1210, 1637, 2060, 2097, 2192, 2262,
    2324, 2394, 2456, 3178,
];

fn div(a: i32, b: i32) -> i32 {
    // Truncating division, matching jalaali-js `~~(a/b)`.
    a / b
}

fn rem(a: i32, b: i32) -> i32 {
    // Truncated remainder, matching jalaali-js `mod`.
    a - (a / b) * b
}

/// Julian Day Number from a Gregorian date (proleptic).
fn g2d(gy: i32, gm: i32, gd: i32) -> i32 {
    let mut d = div((gy + div(gm - 8, 6) + 100100) * 1461, 4)
        + div(153 * rem(gm + 9, 12) + 2, 5)
        + gd
        - 34840408;
    d = d - div(div(gy + 100100 + div(gm - 8, 6), 100) * 3, 4) + 752;
    d
}

/// Gregorian date from a Julian Day Number.
fn d2g(jdn: i32) -> (i32, i32, i32) {
    let mut j = 4 * jdn + 139361631;
    j += div(div(4 * jdn + 183187720, 146097) * 3, 4) * 4 - 3908;
    let i = div(rem(j, 1461), 4) * 5 + 308;
    let gd = div(rem(i, 153), 5) + 1;
    let gm = rem(div(i, 153), 12) + 1;
    let gy = div(j, 1461) - 100100 + div(8 - gm, 6);
    (gy, gm, gd)
}

/// Calendar parameters for one Jalali year.
struct JalCal {
    /// `0` when the year is a leap year.
    leap: i32,
    /// Corresponding Gregorian year of 1 Farvardin.
    gy: i32,
    /// March day on which 1 Farvardin falls.
    march: i32,
}

fn jal_cal(jy: i32) -> JalCal {
    assert!(
        jy >= MIN_JALALI_YEAR && jy < MAX_JALALI_YEAR,
        "Jalali year {jy} out of supported range"
    );
    let bl = BREAKS.len();
    let gy = jy + 621;
    let mut leap_j = -14;
    let mut jp = BREAKS[0];
    let mut jump = 0;
    let mut i = 1;
    while i < bl {
        let jm = BREAKS[i];
        jump = jm - jp;
        if jy < jm {
            break;
        }
        leap_j += div(jump, 33) * 8 + div(rem(jump, 33), 4);
        jp = jm;
        i += 1;
    }
    let mut n = jy - jp;
    leap_j += div(n, 33) * 8 + div(rem(n, 33) + 3, 4);
    if rem(jump, 33) == 4 && jump - n == 4 {
        leap_j += 1;
    }
    let leap_g = div(gy, 4) - div((div(gy, 100) + 1) * 3, 4) - 150;
    let march = 20 + leap_j - leap_g;
    if jump - n < 6 {
        n = n - jump + div(jump + 4, 33) * 33;
    }
    let mut leap = rem(rem(n + 1, 33) - 1, 4);
    if leap == -1 {
        leap = 4;
    }
    JalCal { leap, gy, march }
}

/// Days in month, `jm` in 1..=12.
pub fn month_length(jy: i32, jm: u32) -> u32 {
    match jm {
        1..=6 => 31,
        7..=11 => 30,
        12 => {
            if is_leap_jalali(jy) {
                30
            } else {
                29
            }
        }
        _ => 0,
    }
}

/// True when `jy` is a leap year (Esfand has 30 days).
#[must_use]
pub fn is_leap_jalali(jy: i32) -> bool {
    jal_cal(jy).leap == 0
}

/// A date in the Jalali calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct JalaliDate {
    pub year: i32,
    /// 1..=12
    pub month: u32,
    /// 1..=31
    pub day: u32,
}

impl JalaliDate {
    /// Construct with validation.
    pub fn new(year: i32, month: u32, day: u32) -> Option<JalaliDate> {
        if month < 1 || month > 12 || day < 1 || day > month_length(year, month) {
            return None;
        }
        Some(JalaliDate { year, month, day })
    }

    /// Convert from a Gregorian date.
    #[must_use]
    pub fn from_gregorian(date: NaiveDate) -> JalaliDate {
        let jdn = g2d(date.year(), date.month() as i32, date.day() as i32);
        Self::from_jdn(jdn)
    }

    fn from_jdn(jdn: i32) -> JalaliDate {
        let (gy, _, _) = d2g(jdn);
        let mut jy = gy - 621;
        let r = jal_cal(jy);
        let jdn1f = g2d(r.gy, 3, r.march);
        let mut k = jdn - jdn1f;
        if k >= 0 {
            if k <= 185 {
                let jm = 1 + div(k, 31);
                let jd = rem(k, 31) + 1;
                return JalaliDate { year: jy, month: jm as u32, day: jd as u32 };
            }
            k -= 186;
        } else {
            jy -= 1;
            k += 179;
            if r.leap == 1 {
                k += 1;
            }
        }
        let jm = 7 + div(k, 30);
        let jd = rem(k, 30) + 1;
        JalaliDate { year: jy, month: jm as u32, day: jd as u32 }
    }

    /// Convert back to a Gregorian date.
    #[must_use]
    pub fn to_gregorian(&self) -> NaiveDate {
        let jdn = self.to_jdn();
        let (gy, gm, gd) = d2g(jdn);
        NaiveDate::from_ymd_opt(gy, gm as u32, gd as u32)
            .expect("algorithm produces valid Gregorian dates")
    }

    fn to_jdn(&self) -> i32 {
        let r = jal_cal(self.year);
        let m = self.month as i32;
        g2d(r.gy, 3, r.march)
            + (m - 1) * 31
            - div(m, 7) * (m - 7)
            + self.day as i32
            - 1
    }

    /// Persian month names (L-5 correct typography).
    #[must_use]
    pub fn month_name_fa(&self) -> &'static str {
        const MONTHS_FA: [&str; 12] = [
            "فروردین", "اردیبهشت", "خرداد", "تیر", "مرداد", "شهریور",
            "مهر", "آبان", "آذر", "دی", "بهمن", "اسفند",
        ];
        MONTHS_FA[(self.month - 1) as usize]
    }

    /// Transliterated month names for English UIs showing Jalali dates.
    #[must_use]
    pub fn month_name_en(&self) -> &'static str {
        const MONTHS_EN: [&str; 12] = [
            "Farvardin", "Ordibehesht", "Khordad", "Tir", "Mordad", "Shahrivar",
            "Mehr", "Aban", "Azar", "Dey", "Bahman", "Esfand",
        ];
        MONTHS_EN[(self.month - 1) as usize]
    }
}

impl std::fmt::Display for JalaliDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}/{:02}/{:02}", self.year, self.month, self.day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn known_nowruz_anchors() {
        // Nowruz dates (verified public knowledge):
        assert_eq!(JalaliDate::from_gregorian(g(2024, 3, 20)), JalaliDate::new(1403, 1, 1).unwrap());
        assert_eq!(JalaliDate::from_gregorian(g(2025, 3, 21)), JalaliDate::new(1404, 1, 1).unwrap());
        assert_eq!(JalaliDate::from_gregorian(g(2026, 3, 21)), JalaliDate::new(1405, 1, 1).unwrap());
        assert_eq!(JalaliDate::from_gregorian(g(2022, 3, 21)), JalaliDate::new(1401, 1, 1).unwrap());
    }

    #[test]
    fn known_date_pairs() {
        // Today's date in this environment: 21 Nov 2026 = 30 Aban 1405.
        assert_eq!(
            JalaliDate::from_gregorian(g(2026, 11, 21)),
            JalaliDate::new(1405, 8, 30).unwrap()
        );
        // Last day of leap year 1403, and of non-leap year 1404.
        assert_eq!(
            JalaliDate::from_gregorian(g(2025, 3, 20)),
            JalaliDate::new(1403, 12, 30).unwrap()
        );
        assert_eq!(
            JalaliDate::from_gregorian(g(2026, 3, 20)),
            JalaliDate::new(1404, 12, 29).unwrap()
        );
    }

    #[test]
    fn reverse_conversion() {
        assert_eq!(JalaliDate::new(1405, 1, 1).unwrap().to_gregorian(), g(2026, 3, 21));
        assert_eq!(JalaliDate::new(1403, 12, 30).unwrap().to_gregorian(), g(2025, 3, 20));
        assert_eq!(JalaliDate::new(1405, 8, 30).unwrap().to_gregorian(), g(2026, 11, 21));
    }

    #[test]
    fn leap_years() {
        assert!(is_leap_jalali(1399));
        assert!(is_leap_jalali(1403));
        assert!(!is_leap_jalali(1404));
        assert!(!is_leap_jalali(1405));
        assert_eq!(month_length(1403, 12), 30);
        assert_eq!(month_length(1404, 12), 29);
        assert!(JalaliDate::new(1404, 12, 30).is_none());
    }

    #[test]
    fn round_trip_every_day_2020_2035() {
        let start = g(2020, 1, 1);
        let end = g(2035, 1, 1);
        let mut d = start;
        let mut count = 0;
        while d < end {
            let j = JalaliDate::from_gregorian(d);
            assert_eq!(j.to_gregorian(), d, "round trip failed for {d} -> {j}");
            d = d.succ_opt().unwrap();
            count += 1;
        }
        assert!(count > 5000);
    }

    #[test]
    fn month_names() {
        let j = JalaliDate::new(1405, 8, 30).unwrap();
        assert_eq!(j.month_name_fa(), "آبان");
        assert_eq!(j.month_name_en(), "Aban");
    }
}
