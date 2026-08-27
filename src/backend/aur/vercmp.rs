//! pacman-compatible version comparison (`vercmp`).
//!
//! This is a faithful reimplementation of pacman's well-specified
//! `rpmvercmp`-derived algorithm — a pure function, not dependency
//! resolution. It is used to decide whether an AUR package has an update.

/// Compares two pacman-style version strings (including `epoch:` and
/// `-pkgrel` suffixes).
pub fn vercmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let (epoch_a, rest_a) = split_epoch(a);
    let (epoch_b, rest_b) = split_epoch(b);
    match epoch_a.cmp(&epoch_b) {
        Ordering::Equal => {}
        other => return other,
    }

    let mut pa = rest_a;
    let mut pb = rest_b;

    loop {
        // Skip separators.
        pa = skip_separators(pa);
        pb = skip_separators(pb);
        if pa.is_empty() && pb.is_empty() {
            return Ordering::Equal;
        }

        let num_a = starts_digit(pa);
        let num_b = starts_digit(pb);
        if num_a && !num_b {
            return Ordering::Greater; // numeric segments beat alphabetic ones
        }
        if !num_a && num_b {
            return Ordering::Less;
        }

        if num_a {
            let da = take_while(pa, |c| c.is_ascii_digit());
            let db = take_while(pb, |c| c.is_ascii_digit());
            match compare_numeric(da.0, db.0) {
                Ordering::Equal => {}
                other => return other,
            }
            pa = da.1;
            pb = db.1;
        } else {
            let la = take_while(pa, |c| c.is_ascii_alphabetic());
            let lb = take_while(pb, |c| c.is_ascii_alphabetic());
            match la.0.cmp(lb.0) {
                Ordering::Equal => {}
                other => return other,
            }
            pa = la.1;
            pb = lb.1;
        }
    }
}

fn split_epoch(v: &str) -> (u64, &str) {
    if let Some((e, rest)) = v.split_once(':') {
        if !e.is_empty() && e.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(epoch) = e.parse::<u64>() {
                return (epoch, rest);
            }
        }
    }
    (0, v)
}

fn starts_digit(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn skip_separators(s: &str) -> &str {
    s.trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
}

fn take_while(s: &str, pred: impl Fn(char) -> bool) -> (&str, &str) {
    for (idx, c) in s.char_indices() {
        if !pred(c) {
            return (&s[..idx], &s[idx..]);
        }
    }
    (s, "")
}

fn compare_numeric(a: &str, b: &str) -> std::cmp::Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// True when `newer` is strictly newer than `current`.
pub fn is_newer(current: &str, newer: &str) -> bool {
    vercmp(newer, current) == std::cmp::Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;
    use std::cmp::Ordering::*;

    fn cmp(a: &str, b: &str) -> Ordering {
        vercmp(a, b)
    }

    #[test]
    fn basic_versions() {
        assert_eq!(cmp("1.0", "1.0"), Equal);
        assert_eq!(cmp("6.0", "6.0.1"), Less);
        assert_eq!(cmp("10.0", "9.9"), Greater);
        assert_eq!(cmp("133.0-2", "134.0-1"), Less);
        assert_eq!(cmp("1.0-1", "1.0-2"), Less);
    }

    #[test]
    fn leading_zeros_are_ignored() {
        assert_eq!(cmp("1.01", "1.1"), Equal);
        assert_eq!(cmp("1.001", "1.1"), Equal);
        assert_eq!(cmp("01", "1"), Equal);
    }

    #[test]
    fn alpha_suffixes() {
        assert_eq!(cmp("1.0a", "1.0b"), Less);
        assert_eq!(
            cmp("1.0a", "1.0"),
            Greater,
            "alpha suffix beats plain release"
        );
        // Documented pacman quirk: an rc tag sorts *after* the final release
        // because it is a plain alphabetic suffix. AUR maintainers work
        // around this manually; we mirror pacman's behaviour exactly.
        assert_eq!(cmp("1.0rc1", "1.0"), Greater);
        assert_eq!(cmp("6.11.4.arch1-1", "6.12.1.arch1-1"), Less);
    }

    #[test]
    fn epochs_dominate() {
        assert_eq!(cmp("4:1.0", "3:9.9"), Greater);
        assert_eq!(cmp("2:1.0", "1.9"), Greater);
        assert_eq!(cmp("1:0", "999"), Greater);
    }

    #[test]
    fn separators_are_skipped() {
        assert_eq!(cmp("0.5~git1", "0.5"), Greater);
        assert_eq!(cmp("1.0+really2", "1.0"), Greater);
    }

    #[test]
    fn missing_segment_is_zero() {
        assert_eq!(cmp("1.0", "1.0.0"), Less);
        assert_eq!(cmp("", ""), Equal);
    }

    #[test]
    fn is_newer_helper() {
        assert!(is_newer("2.0-1", "2.1-1"));
        assert!(!is_newer("2.1-1", "2.0-1"));
        assert!(!is_newer("2.1-1", "2.1-1"));
    }
}
