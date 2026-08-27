//! Input validation for identifiers that will be passed to external tools.

/// Characters permitted in package names / ids that we hand to external
/// programs. This is deliberately conservative: it covers pacman, Flatpak and
/// AUR identifiers while rejecting anything shell- or option-like.
const ID_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789@._+-:/";

/// Returns `true` if `name` is safe to pass as a single argv element to an
/// external tool.
///
/// The process API we use never invokes a shell, so this is defence in
/// depth: it additionally guarantees the value cannot be mistaken for an
/// option (no leading `-`) and contains no whitespace, globs or separators.
pub fn valid_identifier(name: &str) -> bool {
    if name.is_empty() || name.len() > 256 {
        return false;
    }
    if name.starts_with('-') {
        return false;
    }
    name.chars().all(|c| ID_CHARS.contains(c))
}

/// Validates and returns a reference to `name`, or an error describing why it
/// was rejected. Every backend must call this before handing user input to an
/// external tool.
pub fn ensure_valid_identifier(name: &str) -> crate::error::Result<()> {
    use crate::error::ForgeError;
    if valid_identifier(name) {
        Ok(())
    } else {
        Err(ForgeError::InvalidInput(format!(
            "'{}' is not a valid package identifier",
            truncate_for_message(name)
        )))
    }
}

fn truncate_for_message(s: &str) -> String {
    if s.chars().count() <= 40 {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(37).collect();
        out.push_str("...");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_names() {
        assert!(valid_identifier("firefox"));
        assert!(valid_identifier("org.mozilla.firefox"));
        assert!(valid_identifier("python-aiohttp_3"));
        assert!(valid_identifier("lib32-mesa"));
        assert!(valid_identifier("yay-bin"));
        assert!(valid_identifier("gtk4+demo"));
        assert!(valid_identifier("1password"));
        // ref with branch, e.g. flatpak app/x/y/z
        assert!(valid_identifier("app/org.mozilla.firefox/x86_64/stable"));
        assert!(valid_identifier("pkg:epoch"));
    }

    #[test]
    fn rejects_option_like_and_shell_like_input() {
        assert!(!valid_identifier("--force"));
        assert!(!valid_identifier("-Syu"));
        assert!(!valid_identifier(""));
        assert!(!valid_identifier("a b"));
        assert!(!valid_identifier("a;b"));
        assert!(!valid_identifier("$(id)"));
        assert!(!valid_identifier("`id`"));
        assert!(!valid_identifier("a\tb"));
        assert!(!valid_identifier("a\nb"));
        assert!(!valid_identifier("*"));
        assert!(!valid_identifier("a'b"));
        assert!(!valid_identifier("a\"b"));
        assert!(!valid_identifier("a|b"));
        assert!(!valid_identifier("a&b"));
        assert!(!valid_identifier("héllo"));
    }

    #[test]
    fn rejects_overlong_input() {
        let long = "a".repeat(257);
        assert!(!valid_identifier(&long));
        let ok = "a".repeat(256);
        assert!(valid_identifier(&ok));
    }

    #[test]
    fn ensure_matches_valid() {
        assert!(ensure_valid_identifier("firefox").is_ok());
        let err = ensure_valid_identifier("--noconfirm").unwrap_err();
        assert!(err.to_string().contains("not a valid package identifier"));
    }
}
