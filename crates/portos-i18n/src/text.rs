//! Bidirectional text and Persian character handling (PRD L-2, L-5, L-6).
//!
//! Technical identifiers (ISO 6346 container numbers, IMO numbers,
//! UN/LOCODE) stay in Latin script and must render LTR inside RTL Persian
//! text — [`wrap_ltr_runs`] inserts Unicode directional isolates so mixed
//! content is unambiguous on any rendering stack.

/// True when the string contains characters from an RTL script
/// (Arabic/Persian, Hebrew, etc.).
#[must_use]
pub fn contains_rtl(s: &str) -> bool {
    s.chars().any(is_rtl_char)
}

#[must_use]
fn is_rtl_char(c: char) -> bool {
    matches!(c as u32,
        0x0590..=0x05FF      // Hebrew
        | 0x0600..=0x06FF    // Arabic
        | 0x0700..=0x074F    // Syriac
        | 0x0750..=0x077F    // Arabic supplement
        | 0x08A0..=0x08FF    // Arabic extended-A
        | 0xFB50..=0xFDFF    // Arabic presentation forms-A
        | 0xFE70..=0xFEFF    // Arabic presentation forms-B
        | 0x200F             // RLM
        | 0x202B             // RLE
        | 0x202E             // RLO
    )
}

/// True when a char belongs to an LTR "technical identifier" run:
/// ASCII alphanumerics plus characters that typically appear inside
/// container/voyage codes.
#[must_use]
fn is_ltr_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | '.' | '_' | '+')
}

fn is_zwj_zwnj_or_space(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}' | '\u{00A0}' | ' ')
}

fn flush_run(out: &mut String, run: &mut String) {
    if run.is_empty() {
        return;
    }
    let trimmed_len = run.trim_end_matches(is_zwj_zwnj_or_space).len();
    let (core, tail) = run.split_at(trimmed_len);
    if !core.is_empty() && core.chars().any(is_ltr_id_char) {
        out.push('\u{2066}'); // LRI
        out.push_str(core);
        out.push('\u{2069}'); // PDI
    } else {
        out.push_str(core);
    }
    out.push_str(tail);
    run.clear();
}

/// Wrap maximal runs of Latin/technical characters in Unicode directional
/// isolates (LRI … PDI) so they display in logical order inside RTL text.
///
/// Example: `"پذیرش MSKU3881107 انجام شد"` → the Latin code is wrapped so
/// it renders LTR inside the Persian sentence.
#[must_use]
pub fn wrap_ltr_runs(text: &str) -> String {
    if !contains_rtl(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 16);
    let mut run = String::new();
    for (i, ch) in chars.iter().copied().enumerate() {
        let keep = is_ltr_id_char(ch)
            || (!run.is_empty()
                && is_zwj_zwnj_or_space(ch)
                && chars.get(i + 1).copied().map_or(false, is_ltr_id_char));
        if keep {
            run.push(ch);
        } else {
            flush_run(&mut out, &mut run);
            out.push(ch);
        }
    }
    flush_run(&mut out, &mut run);
    out
}

/// Normalize Persian/Arabic character variants for storage-agnostic
/// comparison (PRD L-5): Arabic Yeh (ي) → Farsi Yeh (ی), Arabic Kaf (ك) →
/// Keheh (ک), Arabic-Indic digits → Western. Display text is not altered.
#[must_use]
pub fn normalize_persian_chars(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{064A}' => '\u{06CC}', // ي → ی
            '\u{0643}' => '\u{06A9}', // ك → ک
            '\u{0660}'..='\u{0669}' => {
                char::from_u32(u32::from('0') + u32::from(c) - 0x0660).unwrap_or(c)
            }
            '\u{06F0}'..='\u{06F9}' => {
                char::from_u32(u32::from('0') + u32::from(c) - 0x06F0).unwrap_or(c)
            }
            _ => c,
        })
        .collect()
}

/// Collation/search key (PRD L-6): variant-insensitive and
/// diacritic-insensitive, suitable for `ORDER BY`/`LIKE` on the normalized
/// form. ZWNJ is removed so "می‌شود" and "میشود" match.
#[must_use]
pub fn collation_key(s: &str) -> String {
    let normalized = normalize_persian_chars(s);
    normalized
        .chars()
        .filter(|c| {
            // Strip ZWNJ/ZWJ, Arabic diacritics (tashkil), tatweel.
            !matches!(c,
                '\u{200C}' | '\u{200D}'
                | '\u{0640}' | '\u{064B}'..='\u{065F}' | '\u{0670}'
            )
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtl_detection() {
        assert!(contains_rtl("گیت ورودی"));
        assert!(contains_rtl("mixed کت"));
        assert!(!contains_rtl("MSKU3881107 gate-in"));
    }

    #[test]
    fn ltr_runs_are_isolated() {
        let src = "پذیرش MSKU3881107 انجام شد";
        let wrapped = wrap_ltr_runs(src);
        assert!(wrapped.contains("\u{2066}MSKU3881107\u{2069}"));
        // Pure LTR text is untouched.
        assert_eq!(wrap_ltr_runs("MSKU3881107 gate-in"), "MSKU3881107 gate-in");
    }

    #[test]
    fn persian_variants_normalize() {
        // Arabic Yeh/Kaf typed on an Arabic keyboard must equal Farsi chars.
        assert_eq!(normalize_persian_chars("شركت"), "شرکت");
        assert_eq!(normalize_persian_chars("سازي"), "سازی");
        assert_eq!(collation_key("شرکت بازرگانی"), collation_key("شركت بازرگاني"));
    }

    #[test]
    fn collation_ignores_zwnj_and_diacritics() {
        assert_eq!(collation_key("می‌شود"), collation_key("میشود"));
        assert_eq!(collation_key("بَندَر"), collation_key("بندر"));
    }

    #[test]
    fn collation_is_ascii_case_insensitive() {
        assert_eq!(collation_key("msku3881107"), collation_key("MSKU3881107"));
    }
}
