//! Bounce evaluation at RCPT TO time.
//!
//! Patterns are simple globs: `*` matches any run of characters (including
//! `@`). Exact-string matches and `*@domain.test` / `bounce@*` cover the
//! 99% case; this is not full regex.

use crate::db::bounce_rules::BounceRule;
use crate::smtp::response::SmtpReply;

#[derive(Debug, Clone, Default)]
pub struct BounceEvaluator {
    rules: Vec<BounceRule>,
}

impl BounceEvaluator {
    pub fn new(rules: Vec<BounceRule>) -> Self {
        Self { rules }
    }

    pub fn replace(&mut self, rules: Vec<BounceRule>) {
        self.rules = rules;
    }

    pub fn match_recipient(&self, address: &str) -> Option<SmtpReply> {
        let lower = address.to_ascii_lowercase();
        self.rules
            .iter()
            .filter(|r| r.enabled)
            .find(|r| glob_match(&r.address_pattern.to_ascii_lowercase(), &lower))
            .map(|r| SmtpReply::custom(r.smtp_code, r.smtp_message.clone()))
    }
}

/// Tiny glob matcher: `*` is the only wildcard.
pub fn glob_match(pattern: &str, s: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = s.chars().collect();
    fn go(p: &[char], t: &[char]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some('*'), _) => {
                // Greedy match
                for i in 0..=t.len() {
                    if go(&p[1..], &t[i..]) {
                        return true;
                    }
                }
                false
            }
            (Some(a), Some(b)) if a == b => go(&p[1..], &t[1..]),
            _ => false,
        }
    }
    go(&p, &t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob() {
        assert!(glob_match("a@b.com", "a@b.com"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("bounce@*", "bounce@anywhere"));
        assert!(glob_match("*@bouncing-domain.test", "a@bouncing-domain.test"));
        assert!(!glob_match("bounce@*", "real@somewhere"));
    }
}
