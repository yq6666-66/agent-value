//! Privacy rules (v22 §8).
//!
//! The primary guarantee is structural: no record type in this crate has a
//! field for an API key value, a prompt or code text. The checks below are a
//! second, heuristic line of defence against a key value pasted into a field
//! meant for something else (for example the env var *name*).

use crate::error::{CoreError, Result};

/// §8 default retention for task-level data: 180 days.
pub const RETENTION_DAYS: u32 = 180;
/// Maximum length of a probe evidence summary, in characters.
pub const MAX_EVIDENCE_SUMMARY_CHARS: usize = 1000;

/// Prefixes of well-known API key and token formats.
const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "sk_",
    "rk_",
    "pk_live_",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "AKIA",
    "ASIA",
    "AIza",
    "eyJ",
    "-----BEGIN",
];

/// True if some token of `s` (split on whitespace, quotes, brackets and
/// `,;=:`) starts with a well-known key or token prefix followed by at least
/// four more characters.
pub fn has_secret_prefix(s: &str) -> bool {
    s.split(|c: char| c.is_whitespace() || "\"'`()[]{}<>,;=:".contains(c))
        .any(|t| {
            SECRET_PREFIXES
                .iter()
                .any(|p| t.starts_with(p) && t.len() > p.len() + 3)
        })
}

/// Heuristic: does `token` look like a key value rather than a name?
///
/// True for well-known prefixes, and for long (>= 20 chars) tokens that mix
/// upper case, lower case and digits, or that mix letters and digits without
/// any `_` separator.
pub fn looks_like_secret(token: &str) -> bool {
    if has_secret_prefix(token) {
        return true;
    }
    if token.chars().count() < 20 {
        return false;
    }
    let upper = token.chars().any(|c| c.is_ascii_uppercase());
    let lower = token.chars().any(|c| c.is_ascii_lowercase());
    let digit = token.chars().any(|c| c.is_ascii_digit());
    let alpha = upper || lower;
    (upper && lower && digit) || (alpha && digit && !token.contains('_'))
}

/// The name of the environment variable a route reads its key from.
/// Only the name is ever stored; the value is never read into the store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyEnvVarName(String);

impl KeyEnvVarName {
    /// Accepts `^[A-Z][A-Z0-9_]{1,63}$` containing at least one `_`, that
    /// does not look like a key value.
    pub fn new(name: &str) -> Result<Self> {
        if name.is_empty() {
            return Err(CoreError::EmptyField {
                field: "key env var name",
            });
        }
        if looks_like_secret(name) {
            return Err(CoreError::LooksLikeSecret {
                field: "key env var name",
            });
        }
        let mut chars = name.chars();
        let first_ok = chars.next().is_some_and(|c| c.is_ascii_uppercase());
        let rest_ok = chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !first_ok || !rest_ok || name.len() < 2 || name.len() > 64 || !name.contains('_') {
            return Err(CoreError::InvalidField {
                field: "key env var name",
                reason: "must match ^[A-Z][A-Z0-9_]{1,63}$ and contain '_'",
            });
        }
        Ok(Self(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What is kept about a piece of user text (for example the first message
/// of a task): a category, its length, and a keyed digest. Never the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextFacts {
    category: String,
    char_len: u32,
    digest_hex: String,
}

impl TextFacts {
    /// `category` is a short lower-case label (`[a-z0-9-]{1,64}`);
    /// `digest_hex` must be 64 lower-case hex digits (a 256-bit digest).
    pub fn new(category: &str, char_len: u32, digest_hex: &str) -> Result<Self> {
        if category.is_empty()
            || category.len() > 64
            || !category
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(CoreError::InvalidField {
                field: "text category",
                reason: "must match [a-z0-9-]{1,64}",
            });
        }
        if digest_hex.len() != 64
            || !digest_hex
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        {
            return Err(CoreError::InvalidField {
                field: "text digest",
                reason: "must be 64 lower-case hex digits",
            });
        }
        Ok(Self {
            category: category.to_string(),
            char_len,
            digest_hex: digest_hex.to_string(),
        })
    }

    pub fn category(&self) -> &str {
        &self.category
    }
    pub fn char_len(&self) -> u32 {
        self.char_len
    }
    pub fn digest_hex(&self) -> &str {
        &self.digest_hex
    }
}

/// Validates a probe evidence summary: bounded length and no well-known
/// key/token prefixes. Probe ids and response ids are allowed.
pub fn check_evidence_summary(s: &str) -> Result<()> {
    if s.chars().count() > MAX_EVIDENCE_SUMMARY_CHARS {
        return Err(CoreError::OutOfRange {
            what: "evidence summary length",
            value: s.chars().count() as f64,
            min: 0.0,
            max: MAX_EVIDENCE_SUMMARY_CHARS as f64,
        });
    }
    if has_secret_prefix(s) {
        return Err(CoreError::LooksLikeSecret {
            field: "evidence summary",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_names_accept_names_only() {
        for ok in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "RELAY_API_KEY_2",
            "MY_RELAY_KEY",
        ] {
            assert!(KeyEnvVarName::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "openai_api_key",
            "1_KEY",
            "KEY",
            "A-B_C",
            "API KEY",
            &"A_".repeat(40),
        ] {
            assert!(KeyEnvVarName::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn env_var_names_refuse_key_values() {
        // Fake values, split so secret scanners do not flag the source.
        // Fake, low-entropy values built at runtime so secret scanners do not
        // flag the source.
        for v in [
            format!("sk-{}", "a1".repeat(10)),
            format!("AKIA{}", "A".repeat(16)),
            format!("ghp_{}", "aB1".repeat(8)),
            format!("{}{}", "AB".repeat(6), "12".repeat(5)),
        ] {
            assert!(
                matches!(
                    KeyEnvVarName::new(&v),
                    Err(CoreError::LooksLikeSecret { .. })
                ),
                "{v}"
            );
        }
    }

    #[test]
    fn secret_heuristic() {
        // Built at runtime: a low-entropy stand-in with upper, lower and digits.
        assert!(looks_like_secret(&"aB3".repeat(8)));
        assert!(!looks_like_secret("ANTHROPIC_API_KEY_SECONDARY_2024"));
        assert!(!looks_like_secret("short1A"));
        assert!(has_secret_prefix(concat!("token=\"sk-", "abcd1234\" here")));
        assert!(!has_secret_prefix("sk- alone"));
    }

    #[test]
    fn text_facts_validation() {
        let hex = "0".repeat(64);
        assert!(TextFacts::new("first-message", 42, &hex).is_ok());
        assert!(TextFacts::new("First", 42, &hex).is_err());
        assert!(TextFacts::new("", 42, &hex).is_err());
        assert!(TextFacts::new("x", 1, "abc").is_err());
        assert!(TextFacts::new("x", 1, &"G".repeat(64)).is_err());
    }

    #[test]
    fn evidence_summaries() {
        assert!(
            check_evidence_summary("deterministic 9/10 correct; response msg_01ABCdef234").is_ok()
        );
        assert!(check_evidence_summary(concat!("echoed key sk-", "live-abcdef0123")).is_err());
        assert!(check_evidence_summary(&"x".repeat(1001)).is_err());
    }
}
