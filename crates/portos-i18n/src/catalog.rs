//! Translation catalogs (PRD L-10).
//!
//! Catalogs are plain JSON resource files embedded at build time and
//! optionally overridden at runtime from a configured directory, so
//! languages can be updated without a code release. Adding a language
//! later means adding a catalog file — no code changes.

use std::collections::BTreeMap;
use std::path::Path;

use crate::Locale;

/// Interpolation arguments: `Args::new().arg("container", "MSKU3881107")`.
#[derive(Debug, Clone, Default)]
pub struct Args(Vec<(String, String)>);

impl Args {
    #[must_use]
    pub fn new() -> Self {
        Self(Vec::new())
    }

    #[must_use]
    pub fn arg(mut self, name: &str, value: impl Into<String>) -> Self {
        self.0.push((name.to_string(), value.into()));
        self
    }

    fn interpolate(&self, template: &str) -> String {
        let mut out = template.to_string();
        for (name, value) in &self.0 {
            let needle = format!("{{{name}}}");
            out = out.replace(&needle, value);
        }
        out
    }
}

impl FromIterator<(String, String)> for Args {
    fn from_iter<T: IntoIterator<Item = (String, String)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// One language's message catalog (flat key → template string).
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    entries: BTreeMap<String, String>,
}

impl Catalog {
    /// Parse a JSON object of `{"key": "message with {arg}"}`.
    pub fn from_json(json: &str) -> Result<Catalog, String> {
        let parsed: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(json).map_err(|e| format!("catalog json: {e}"))?;
        let mut entries = BTreeMap::new();
        for (k, v) in parsed {
            if let Some(s) = v.as_str() {
                entries.insert(k, s.to_string());
            }
        }
        Ok(Catalog { entries })
    }

    /// Merge another catalog over this one (runtime overrides win).
    pub fn merge(&mut self, other: Catalog) {
        for (k, v) in other.entries {
            self.entries.insert(k, v);
        }
    }

    /// Load a catalog from a JSON file on disk.
    pub fn load_file(path: &Path) -> Result<Catalog, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        Catalog::from_json(&text)
    }

    /// Raw template lookup.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// Interpolated message; missing keys return the key itself.
    #[must_use]
    pub fn t(&self, key: &str, args: &Args) -> String {
        self.get(key)
            .map(|tpl| args.interpolate(tpl))
            .unwrap_or_else(|| key.to_string())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All keys (for tooling / QA completeness checks).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

/// The set of catalogs for all supported locales.
#[derive(Debug, Clone)]
pub struct CatalogSet {
    en: Catalog,
    fa: Catalog,
}

impl CatalogSet {
    /// Embedded default catalogs shipped with the binary.
    ///
    /// # Errors
    /// Never in practice; embedded JSON is validated in CI tests.
    pub fn embedded() -> CatalogSet {
        let en = Catalog::from_json(include_str!("resources/en.json"))
            .expect("embedded en catalog valid");
        let fa = Catalog::from_json(include_str!("resources/fa.json"))
            .expect("embedded fa catalog valid");
        CatalogSet { en, fa }
    }

    /// Merge overrides from a directory of `<locale>.json` files (L-10).
    /// Unknown locale files are ignored with a returned warning.
    pub fn load_overrides(&mut self, dir: &Path) -> Result<Vec<String>, String> {
        let mut warnings = Vec::new();
        let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            match Locale::parse(&stem) {
                Some(Locale::En) => {
                    let cat = Catalog::load_file(&path)?;
                    self.en.merge(cat);
                }
                Some(Locale::Fa) => {
                    let cat = Catalog::load_file(&path)?;
                    self.fa.merge(cat);
                }
                None => warnings.push(format!(
                    "ignored catalog {}: unknown locale {:?}",
                    path.display(),
                    stem
                )),
            }
        }
        Ok(warnings)
    }

    /// Catalog for a locale, falling back to English (functional parity is
    /// enforced by tests, but a missing key must never break the UI).
    #[must_use]
    pub fn catalog(&self, locale: Locale) -> &Catalog {
        match locale {
            Locale::En => &self.en,
            Locale::Fa => &self.fa,
        }
    }

    /// Translate with fallback: requested locale → English → key.
    #[must_use]
    pub fn t(&self, locale: Locale, key: &str, args: &Args) -> String {
        let primary = self.catalog(locale);
        if let Some(tpl) = primary.get(key) {
            return args.interpolate(tpl);
        }
        if locale != Locale::En {
            if let Some(tpl) = self.en.get(key) {
                return args.interpolate(tpl);
            }
        }
        key.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_and_fallback() {
        let set = CatalogSet::embedded();
        let args = Args::new().arg("container", "MSKU3881107");
        let fa = set.t(Locale::Fa, "yard.container.placed", &args);
        assert!(!fa.is_empty());
        assert!(fa.contains("MSKU3881107"), "{fa}");
        // Unknown key falls back to the key itself.
        assert_eq!(
            set.t(Locale::En, "no.such.key", &Args::new()),
            "no.such.key"
        );
    }

    #[test]
    fn embedded_catalogs_have_parity() {
        // PRD §3.2: Persian and English content must have functional parity.
        let set = CatalogSet::embedded();
        let missing: Vec<String> = set
            .en
            .keys()
            .filter(|k| set.fa.get(k).is_none())
            .map(str::to_string)
            .collect();
        assert!(missing.is_empty(), "fa missing keys: {missing:?}");
        let extra: Vec<String> = set
            .fa
            .keys()
            .filter(|k| set.en.get(k).is_none())
            .map(str::to_string)
            .collect();
        assert!(extra.is_empty(), "fa extra keys: {extra:?}");
    }

    #[test]
    fn override_merging() {
        let mut set = CatalogSet::embedded();
        let tmp = std::env::temp_dir().join("portos-test-i18n-override");
        std::fs::create_dir_all(&tmp).unwrap();
        let p = tmp.join("en.json");
        std::fs::write(&p, r#"{"gate.tx.started":"Truck {plate} is at the gate"}"#).unwrap();
        let warns = set.load_overrides(&tmp).unwrap();
        assert!(warns.is_empty(), "{warns:?}");
        let msg = set.t(
            Locale::En,
            "gate.tx.started",
            &Args::new().arg("plate", "12A34567"),
        );
        assert_eq!(msg, "Truck 12A34567 is at the gate");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
