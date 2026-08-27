//! `.SRCINFO` parser (output of `makepkg --printsrcinfo`).
//!
//! Format: `key = value` lines, indented sections per sub-package
//! (`pkgname = ...`). We only need the pkgbase-level metadata.

use std::collections::HashMap;

/// Parsed base-level SRCINFO fields; one entry per occurrence of each key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SrcInfo {
    pub fields: HashMap<String, Vec<String>>,
    /// Sub-package names (`pkgname = ...`) beyond the base package.
    pub split_packages: Vec<String>,
}

impl SrcInfo {
    pub fn first(&self, key: &str) -> Option<&str> {
        self.fields
            .get(key)
            .and_then(|v| v.first())
            .map(|s| s.as_str())
    }

    pub fn values(&self, key: &str) -> &[String] {
        self.fields.get(key).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn description(&self) -> Option<&str> {
        self.first("pkgdesc")
    }

    pub fn depends(&self) -> Vec<String> {
        self.values("depends").to_vec()
    }

    /// make + check dependencies (both are build-time requirements).
    pub fn build_depends(&self) -> Vec<String> {
        let mut all = self.values("makedepends").to_vec();
        all.extend(self.values("checkdepends").iter().cloned());
        all.sort();
        all.dedup();
        all
    }

    pub fn opt_depends(&self) -> Vec<String> {
        self.values("optdepends").to_vec()
    }
}

pub fn parse(text: &str) -> SrcInfo {
    let mut info = SrcInfo::default();
    // Which section we're in: "base" until the first pkgname line.
    let mut in_base_section = true;

    for raw in text.lines() {
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        let line = raw.trim_start();
        let Some((key_raw, value)) = line.split_once('=') else {
            continue;
        };
        let key = key_raw.trim().to_string();
        let value = value.trim().to_string();

        match key.as_str() {
            "pkgname" => {
                in_base_section = false;
                info.split_packages.push(value);
                continue;
            }
            "pkgbase" => {
                in_base_section = true;
                continue;
            }
            _ => {}
        }

        if in_base_section {
            info.fields.entry(key).or_default().push(value);
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
pkgbase = example-tool
	pkgdesc = A tool for examples
	pkgver = 1.2.3
	url = https://example.org/tool
	arch = x86_64
	license = MIT
	depends = gtk4>=4.10
	depends = glib2
	makedepends = cargo
	makedepends = git
	checkdepends = pytest
	optdepends = bash-completion: shell completion

pkgname = example-tool
	depends = gtk4>=4.10

pkgname = example-tool-extra
	pkgdesc = Extra bits
	depends = libextra
";

    #[test]
    fn parses_base_fields() {
        let info = parse(FIXTURE);
        assert_eq!(info.description(), Some("A tool for examples"));
        assert_eq!(info.first("url"), Some("https://example.org/tool"));
        assert_eq!(info.depends(), vec!["gtk4>=4.10", "glib2"]);
        assert_eq!(info.build_depends(), vec!["cargo", "git", "pytest"]);
        assert_eq!(
            info.opt_depends(),
            vec!["bash-completion: shell completion"]
        );
    }

    #[test]
    fn collects_split_packages_and_skips_their_fields() {
        let info = parse(FIXTURE);
        assert_eq!(
            info.split_packages,
            vec!["example-tool", "example-tool-extra"]
        );
        // The split-package-only dep must not leak into base fields.
        assert!(!info.depends().contains(&"libextra".to_string()));
    }

    #[test]
    fn tolerates_comments_and_empty_lines() {
        let info = parse("# comment\n\npkgbase = a\n\tpkgver = 1\n");
        assert_eq!(info.first("pkgver"), Some("1"));
    }

    #[test]
    fn empty_input_is_empty() {
        let info = parse("");
        assert!(info.fields.is_empty());
        assert_eq!(info.description(), None);
    }
}
