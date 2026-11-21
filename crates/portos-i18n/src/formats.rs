//! Locale-aware display formatting (PRD L-3, L-4, L-8, L-9).
//!
//! Dates, datetimes, money and quantities formatted per locale, calendar
//! and numeral-system preference. Persian uses U+066C (thousands) and
//! U+066B (decimal) separators and Persian digits when requested.

use crate::jalali::JalaliDate;
use crate::{Calendar, Locale, Numerals};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use portos_core::money::{Currency, Money};

use crate::numerals::{group_thousands, to_persian_digits};

const FA_THOUSANDS: char = '\u{066C}'; // ٬
const FA_DECIMAL: char = '\u{066B}'; // ٫

fn digits_for(s: &str, numerals: Numerals) -> String {
    match numerals {
        Numerals::Western => s.to_string(),
        Numerals::Persian => to_persian_digits(s),
    }
}

/// Numeric Gregorian date, e.g. `2026-11-21` (or Persian digits).
#[must_use]
pub fn date_gregorian_numeric(d: NaiveDate, numerals: Numerals) -> String {
    digits_for(&d.format("%Y-%m-%d").to_string(), numerals)
}

/// Numeric Jalali date, e.g. `1405/08/30`.
#[must_use]
pub fn date_jalali_numeric(d: NaiveDate, numerals: Numerals) -> String {
    let j = JalaliDate::from_gregorian(d);
    digits_for(&format!("{j}"), numerals)
}

/// Long-form date with month name.
/// English Gregorian: `21 November 2026`; Persian Jalali: `۳۰ آبان ۱۴۰۵`;
/// Persian Gregorian: `۲۱ نوامبر ۲۰۲۶`.
#[must_use]
pub fn date_long(locale: Locale, calendar: Calendar, d: NaiveDate, numerals: Numerals) -> String {
    match (locale, calendar) {
        (Locale::En, Calendar::Gregorian) => {
            digits_for(&d.format("%d %B %Y").to_string(), Numerals::Western)
        }
        (Locale::En, Calendar::Jalali) => {
            let j = JalaliDate::from_gregorian(d);
            format!(
                "{} {} {}",
                j.day,
                j.month_name_en(),
                digits_for(&j.year.to_string(), Numerals::Western)
            )
        }
        (Locale::Fa, Calendar::Jalali) => {
            let j = JalaliDate::from_gregorian(d);
            format!(
                "{} {} {}",
                digits_for(&j.day.to_string(), numerals),
                j.month_name_fa(),
                digits_for(&j.year.to_string(), numerals)
            )
        }
        (Locale::Fa, Calendar::Gregorian) => {
            const FA_MONTHS: [&str; 12] = [
                "ژانویه",
                "فوریه",
                "مارس",
                "آوریل",
                "مه",
                "ژوئن",
                "ژوئیه",
                "اوت",
                "سپتامبر",
                "اکتبر",
                "نوامبر",
                "دسامبر",
            ];
            format!(
                "{} {} {}",
                digits_for(&d.day().to_string(), numerals),
                FA_MONTHS[(d.month0()) as usize],
                digits_for(&d.year().to_string(), numerals)
            )
        }
    }
}

/// A datetime rendered per locale/calendar/numeral preferences; seconds
/// and the timezone label stay unambiguous (`Z` = UTC).
#[must_use]
pub fn datetime_display(
    locale: Locale,
    calendar: Calendar,
    numerals: Numerals,
    dt: DateTime<Utc>,
) -> String {
    let date = match calendar {
        Calendar::Gregorian => date_gregorian_numeric(dt.date_naive(), numerals),
        Calendar::Jalali => date_jalali_numeric(dt.date_naive(), numerals),
    };
    let time = dt.format("%H:%M").to_string();
    let _ = locale; // time format identical in both locales (24h clock)
    format!("{date} {}", digits_for(&time, numerals))
}

/// Money display. IRR renders with the Persian name «ریال» for Persian
/// users and zero decimals; other currencies keep their ISO code.
#[must_use]
pub fn money_display(locale: Locale, numerals: Numerals, money: Money) -> String {
    let dec = money.currency.decimals() as usize;
    let scale = 10i64.pow(dec as u32);
    let sign = if money.minor < 0 { "-" } else { "" };
    let abs = money.minor.unsigned_abs();
    let int_part = group_thousands(
        &(abs / scale as u64).to_string(),
        if numerals == Numerals::Persian {
            FA_THOUSANDS
        } else {
            ','
        },
    );
    let body = match dec {
        0 => int_part,
        _ => {
            let frac = format!("{:0width$}", abs % scale as u64, width = dec);
            let decimal_sep = if numerals == Numerals::Persian {
                FA_DECIMAL
            } else {
                '.'
            };
            format!("{int_part}{decimal_sep}{frac}")
        }
    };
    let unit = match (locale, money.currency) {
        (Locale::Fa, Currency::IRR) => "ریال".to_string(),
        (Locale::Fa, Currency::USD) => "دلار آمریکا".to_string(),
        (Locale::Fa, Currency::EUR) => "یورو".to_string(),
        (Locale::Fa, Currency::AED) => "درهم امارات".to_string(),
        (Locale::Fa, Currency::TRY) => "لیر ترکیه".to_string(),
        (Locale::Fa, Currency::CNY) => "یوان چین".to_string(),
        (_, cur) => cur.code().to_string(),
    };
    format!("{sign}{} {unit}", digits_for(&body, numerals))
}

/// Toman display for Rial amounts (1 Toman = 10 Rial). Presentational
/// only — storage remains Rial minor units (PRD L-9).
#[must_use]
pub fn toman_display(locale: Locale, numerals: Numerals, rial: Money) -> Option<String> {
    if rial.currency != Currency::IRR {
        return None;
    }
    let tomans = rial.minor / 10;
    let m = Money::new(tomans, Currency::IRR);
    let s = money_display(locale, numerals, m);
    Some(if locale == Locale::Fa {
        s.replace("ریال", "تومان")
    } else {
        s.replace("IRR", "Toman")
    })
}

/// Quantity with fixed decimals and locale separators.
#[must_use]
pub fn number_display(numerals: Numerals, value: f64, decimals: u32) -> String {
    let formatted = format!("{value:.0$}", decimals as usize);
    let (int_part, frac_part) = formatted
        .split_once('.')
        .unwrap_or((formatted.as_str(), ""));
    let int_part = group_thousands(
        int_part,
        if numerals == Numerals::Persian {
            FA_THOUSANDS
        } else {
            ','
        },
    );
    let body = if frac_part.is_empty() {
        int_part
    } else if numerals == Numerals::Persian {
        format!("{int_part}{FA_DECIMAL}{frac_part}")
    } else {
        format!("{int_part}.{frac_part}")
    };
    digits_for(&body, numerals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn jalali_display_of_known_date() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        assert_eq!(date_jalali_numeric(d, Numerals::Western), "1405/08/30");
        assert_eq!(date_jalali_numeric(d, Numerals::Persian), "۱۴۰۵/۰۸/۳۰");
        let long = date_long(Locale::Fa, Calendar::Jalali, d, Numerals::Persian);
        assert_eq!(long, "۳۰ آبان ۱۴۰۵");
    }

    #[test]
    fn gregorian_display() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        assert_eq!(date_gregorian_numeric(d, Numerals::Western), "2026-11-21");
        assert_eq!(
            date_long(Locale::En, Calendar::Gregorian, d, Numerals::Western),
            "21 November 2026"
        );
    }

    #[test]
    fn money_irr_and_usd() {
        let irr = Money::new(850_000, Currency::IRR);
        assert_eq!(
            money_display(Locale::En, Numerals::Western, irr),
            "850,000 IRR"
        );
        assert_eq!(
            money_display(Locale::Fa, Numerals::Persian, irr),
            "۸۵۰٬۰۰۰ ریال"
        );
        let usd = Money::new(125_050, Currency::USD);
        assert_eq!(
            money_display(Locale::En, Numerals::Western, usd),
            "1,250.50 USD"
        );
        assert_eq!(
            money_display(Locale::Fa, Numerals::Persian, usd),
            "۱٬۲۵۰٫۵۰ دلار آمریکا"
        );
    }

    #[test]
    fn toman_presentation() {
        let irr = Money::new(850_000, Currency::IRR);
        let t = toman_display(Locale::Fa, Numerals::Persian, irr).unwrap();
        assert_eq!(t, "۸۵٬۰۰۰ تومان");
        let usd = Money::new(100, Currency::USD);
        assert!(toman_display(Locale::Fa, Numerals::Persian, usd).is_none());
    }

    #[test]
    fn datetime_display_jalali() {
        let dt = Utc.with_ymd_and_hms(2026, 11, 21, 9, 5, 0).unwrap();
        let s = datetime_display(Locale::Fa, Calendar::Jalali, Numerals::Persian, dt);
        assert_eq!(s, "۱۴۰۵/۰۸/۳۰ ۰۹:۰۵");
    }

    #[test]
    fn numbers_with_separators() {
        assert_eq!(number_display(Numerals::Western, 12.5, 1), "12.5");
        assert_eq!(number_display(Numerals::Persian, 12345.0, 0), "۱۲٬۳۴۵");
    }
}
