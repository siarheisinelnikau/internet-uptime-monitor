//! Complete embedded interface catalogs. English message text is the stable fallback key.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Russian,
    French,
    German,
    Spanish,
    Italian,
    #[default]
    #[serde(other)]
    English,
}
impl Language {
    pub const ALL: [Self; 6] = [
        Self::English,
        Self::Russian,
        Self::French,
        Self::German,
        Self::Spanish,
        Self::Italian,
    ];
    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Russian => "Русский",
            Self::French => "Français",
            Self::German => "Deutsch",
            Self::Spanish => "Español",
            Self::Italian => "Italiano",
        }
    }
    fn index(self) -> Option<usize> {
        match self {
            Self::English => None,
            Self::Russian => Some(0),
            Self::French => Some(1),
            Self::German => Some(2),
            Self::Spanish => Some(3),
            Self::Italian => Some(4),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    #[default]
    #[serde(other)]
    Light,
}

fn catalog() -> &'static BTreeMap<String, [String; 5]> {
    static CATALOG: OnceLock<BTreeMap<String, [String; 5]>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        include_str!("../assets/locales.psv")
            .lines()
            .map(|line| {
                let mut fields = line.split('|');
                let english = fields.next().expect("message key").to_owned();
                let translations: [String; 5] = fields
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
                    .try_into()
                    .expect("five complete translations");
                (english, translations)
            })
            .collect()
    })
}

pub fn tr(language: Language, english: &'static str) -> &'static str {
    language
        .index()
        .and_then(|index| catalog().get(english).map(|row| row[index].as_str()))
        .unwrap_or(english)
}

/// Replaces positional placeholders without interpreting user-provided values as message keys.
pub fn message(language: Language, english: &'static str, values: &[String]) -> String {
    let template = tr(language, english);
    let mut result = String::new();
    let mut parts = template.split("{}");
    result.push_str(parts.next().unwrap_or_default());
    for (index, part) in parts.enumerate() {
        result.push_str(values.get(index).map(String::as_str).unwrap_or("{}"));
        result.push_str(part);
    }
    result
}

/// Localizes known application diagnostics only; preserves OS/network details verbatim.
pub fn diagnostic(language: Language, detail: &str) -> String {
    if let Some(index) = language.index() {
        if let Some(row) = catalog().get(detail) {
            return row[index].clone();
        }
        for prefix in [
            "Could not save the check:",
            "Tray unavailable:",
            "Update failed:",
            "Could not execute ping:",
        ] {
            if let Some(rest) = detail.strip_prefix(prefix) {
                return format!("{}{}", tr(language, prefix), rest);
            }
        }
        for (prefix, key) in [
            ("No reply (", "No reply ({})"),
            (
                "Cannot open the Windows ICMP driver (",
                "Cannot open the Windows ICMP driver ({})",
            ),
            (
                "Windows could not execute the ICMP probe (",
                "Windows could not execute the ICMP probe ({})",
            ),
        ] {
            if let Some(value) = detail
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(')'))
            {
                return message(language, key, &[value.to_owned()]);
            }
        }
        for (prefix, key) in [
            (
                "Could not open your startup preferences (",
                "Could not open your startup preferences ({}).",
            ),
            (
                "Could not update automatic startup (",
                "Could not update automatic startup ({}).",
            ),
        ] {
            if let Some(value) = detail
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(")."))
            {
                return message(language, key, &[value.to_owned()]);
            }
        }
    }
    detail.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalogs_preserve_placeholders_and_have_no_empty_translations() {
        assert!(catalog().len() > 150);
        assert_eq!(
            catalog().len(),
            include_str!("../assets/locales.psv").lines().count(),
            "duplicate message key"
        );
        for (english, translations) in catalog() {
            let count = english.matches("{}").count();
            for translated in translations {
                assert!(
                    !translated.trim().is_empty(),
                    "empty translation: {english}"
                );
                assert_eq!(
                    translated.matches("{}").count(),
                    count,
                    "placeholder mismatch: {english}"
                );
            }
        }
    }
    #[test]
    fn interpolation_preserves_values_and_unknown_diagnostics() {
        assert_eq!(
            message(
                Language::French,
                "Report saved to {}",
                &["C:/Settings/Online.txt".into()]
            ),
            "Rapport enregistré dans C:/Settings/Online.txt"
        );
        assert_eq!(
            diagnostic(Language::Russian, "C:/Settings/Online.txt"),
            "C:/Settings/Online.txt"
        );
        assert_eq!(
            tr(Language::German, "Unknown future message"),
            "Unknown future message"
        );
    }
}
