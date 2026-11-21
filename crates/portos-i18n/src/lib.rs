//! PortOS localization engine (PRD §3).
//!
//! Full bilingual English/Persian support with functional parity:
//!
//! * resource-file translation catalogs (L-10) with runtime override
//!   directories so languages can be updated without a code release;
//! * the Persian (Jalali / Solar Hijri) calendar alongside Gregorian (L-3),
//!   while storage/APIs stay ISO 8601/UTC;
//! * Persian and Western numerals with input normalization (L-4);
//! * bidirectional text helpers so Latin technical identifiers
//!   (ISO 6346 container numbers, IMO numbers) render correctly inside
//!   RTL Persian text (L-2 / §3.2);
//! * Persian character normalization (ی/ي, ک/ك) and variant-insensitive
//!   collation keys for search and sorting (L-5, L-6);
//! * locale-aware formatting of dates, money (incl. IRR, no decimals) and
//!   quantities (L-9).

pub mod catalog;
pub mod formats;
pub mod jalali;
pub mod numerals;
pub mod text;

pub use catalog::{Args, Catalog, CatalogSet};
pub use jalali::JalaliDate;

use serde::{Deserialize, Serialize};

/// Supported user-facing languages. The architecture allows adding further
/// locales later by shipping an additional catalog file (L-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    #[default]
    En,
    Fa,
}

impl Locale {
    /// BCP-47 language tag.
    pub fn tag(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Fa => "fa",
        }
    }

    /// Human name in its own language.
    pub fn endonym(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::Fa => "فارسی",
        }
    }

    /// Base direction of the writing system (L-2).
    pub fn rtl(self) -> bool {
        matches!(self, Locale::Fa)
    }

    /// Parse from an `Accept-Language` style value, first match wins.
    pub fn parse(tag: &str) -> Option<Locale> {
        let t = tag.trim().to_ascii_lowercase();
        // Accept "fa", "fa-IR", "fa_IR", "persian", "farsi", "en", "en-US", ...
        let base = t.split(['-', '_', ',']).next().unwrap_or("").trim();
        match base {
            "fa" | "fas" | "per" | "persian" | "farsi" => Some(Locale::Fa),
            "en" | "eng" | "english" => Some(Locale::En),
            _ => None,
        }
    }

    /// Resolve from a comma-separated `Accept-Language` header.
    pub fn parse_accept_language(header: &str) -> Option<Locale> {
        header
            .split(',')
            .filter_map(|part| {
                let part = part.trim();
                let tag = part.split(';').next()?.trim();
                Locale::parse(tag)
            })
            .next()
    }
}

/// Calendar preference for *display* only; storage and APIs remain
/// ISO 8601 / Gregorian (L-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Calendar {
    #[default]
    Gregorian,
    Jalali,
}

impl Calendar {
    pub fn parse(s: &str) -> Option<Calendar> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gregorian" | "iso" | "en" => Some(Calendar::Gregorian),
            "jalali" | "persian" | "solar-hijri" | "fa" => Some(Calendar::Jalali),
            _ => None,
        }
    }
}

/// Numeral system preference for display; input accepts both and is
/// normalized to Western digits in the canonical form (L-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Numerals {
    #[default]
    Western,
    Persian,
}

impl Numerals {
    pub fn parse(s: &str) -> Option<Numerals> {
        match s.trim().to_ascii_lowercase().as_str() {
            "western" | "latin" | "en" => Some(Numerals::Western),
            "persian" | "eastern-arabic" | "fa" => Some(Numerals::Persian),
            _ => None,
        }
    }
}

/// Default display preferences per locale (used when a user has not saved
/// an explicit preference).
pub fn defaults_for(locale: Locale) -> (Calendar, Numerals) {
    match locale {
        Locale::En => (Calendar::Gregorian, Numerals::Western),
        // Persian users see the Jalali calendar and Persian digits by
        // default; both are user-preference switchable at any time (L-3/L-4).
        Locale::Fa => (Calendar::Jalali, Numerals::Persian),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_locale_tags() {
        assert_eq!(Locale::parse("fa"), Some(Locale::Fa));
        assert_eq!(Locale::parse("fa-IR"), Some(Locale::Fa));
        assert_eq!(Locale::parse("fa_IR"), Some(Locale::Fa));
        assert_eq!(Locale::parse("Persian"), Some(Locale::Fa));
        assert_eq!(Locale::parse("en-US"), Some(Locale::En));
        assert_eq!(Locale::parse("de"), None);
    }

    #[test]
    fn parses_accept_language() {
        assert_eq!(
            Locale::parse_accept_language("fa-IR,fa;q=0.9,en-US;q=0.8"),
            Some(Locale::Fa)
        );
        assert_eq!(Locale::parse_accept_language("en;q=0.5"), Some(Locale::En));
        assert_eq!(Locale::parse_accept_language("*"), None);
    }

    #[test]
    fn rtl_only_for_persian() {
        assert!(!Locale::En.rtl());
        assert!(Locale::Fa.rtl());
    }
}
