//! Persian and Western numerals (PRD L-4).
//!
//! Display converts to the user's preferred numeral system; input accepts
//! both Persian (۰۱۲۳۴۵۶۷۸۹) and Arabic-Indic (٠١٢٣٤٥٦٧٨٩) digits and is
//! normalized to Western digits as the canonical form.

/// Convert ASCII digits to Persian digits.
#[must_use]
pub fn to_persian_digits(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_digit() {
                char::from_u32(0x06F0 + u32::from(c) - u32::from('0')).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// Convert Persian and Arabic-Indic digits back to ASCII digits.
#[must_use]
pub fn to_western_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{06F0}'..='\u{06F9}' => {
                char::from_u32(u32::from('0') + u32::from(c) - 0x06F0).unwrap_or(c)
            }
            '\u{0660}'..='\u{0669}' => {
                char::from_u32(u32::from('0') + u32::from(c) - 0x0660).unwrap_or(c)
            }
            _ => c,
        })
        .collect()
}

/// Canonical input normalization: Persian/Arabic-Indic digits → Western.
/// Persian thousands (٬ U+066C) and decimal (٫ U+066B) separators are
/// normalized to `,` and `.`.
#[must_use]
pub fn normalize_numerals(s: &str) -> String {
    to_western_digits(s)
        .replace('\u{066C}', ",")
        .replace('\u{066B}', ".")
}

/// Insert thousands separators every 3 digits into an integer string.
#[must_use]
pub fn group_thousands(digits: &str, sep: char) -> String {
    let neg = digits.starts_with('-');
    let body = digits.trim_start_matches('-');
    let grouped = body
        .chars()
        .rev()
        .collect::<Vec<_>>()
        .chunks(3)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(&sep.to_string())
        .chars()
        .rev()
        .collect::<String>();
    if neg {
        format!("-{grouped}")
    } else {
        grouped
    }
}

/// Parse an integer accepting Persian, Arabic-Indic and Western digits.
pub fn parse_i64(s: &str) -> Result<i64, String> {
    let normalized = normalize_numerals(s).replace(',', "");
    normalized
        .parse::<i64>()
        .map_err(|e| format!("{s:?}: {e}"))
}

/// Parse a decimal number accepting both digit systems and both
/// decimal separators.
pub fn parse_f64(s: &str) -> Result<f64, String> {
    let normalized = normalize_numerals(s).replace(',', "");
    normalized
        .parse::<f64>()
        .map_err(|e| format!("{s:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_conversion_round_trip() {
        assert_eq!(to_persian_digits("1405/08/30"), "۱۴۰۵/۰۸/۳۰");
        assert_eq!(to_western_digits("۱۴۰۵/۰۸/۳۰"), "1405/08/30");
        assert_eq!(to_western_digits("٠١٢٣"), "0123");
        assert_eq!(to_western_digits(&to_persian_digits("2026-11-21")), "2026-11-21");
    }

    #[test]
    fn normalization_of_input() {
        assert_eq!(normalize_numerals("۸۵۰٬۰۰۰"), "850,000");
        assert_eq!(normalize_numerals("85٫5"), "85.5");
    }

    #[test]
    fn grouping() {
        assert_eq!(group_thousands("850000", ','), "850,000");
        assert_eq!(group_thousands("1234567", '\u{066C}'), "1\u{066C}234\u{066C}567");
        assert_eq!(group_thousands("-9876", ','), "-9,876");
    }

    #[test]
    fn parsing_mixed_digit_systems() {
        assert_eq!(parse_i64("۱۲۳۴").unwrap(), 1234);
        assert_eq!(parse_i64("٠٩").unwrap(), 9);
        assert_eq!(parse_f64("۸٫۵").unwrap(), 8.5);
        assert_eq!(parse_i64("1,234").unwrap(), 1234);
        assert!(parse_i64("abc").is_err());
    }
}
