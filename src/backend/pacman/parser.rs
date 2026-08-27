//! Parsers for pacman's machine-stable output formats.
//!
//! Forge never parses ANSI-coloured terminal decoration (`--color never`
//! is always passed) and only consumes formats that are either explicitly
//! scriptable (`--print-format`) or field-aligned key/value blocks that have
//! been stable across pacman's history. Decisions are never made from prose;
//! resolution always comes from pacman itself.

/// One row of `pacman -Ss --color never`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncSearchRow {
    pub repo: String,
    pub name: String,
    pub version: String,
    /// `Some(version)` when marked `[installed]`, `[installed: other-repo]`.
    pub installed_version: Option<String>,
    pub description: String,
}

pub fn parse_search_output(text: &str) -> Vec<SyncSearchRow> {
    let mut rows = Vec::new();
    let mut current: Option<SyncSearchRow> = None;

    for raw in text.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            // Description continuation.
            if let Some(mut c) = current.take() {
                if c.description.is_empty() {
                    c.description = line.trim().to_string();
                }
                rows.push(c);
            }
            continue;
        }
        // Flush previous row that had no description line.
        if let Some(c) = current.take() {
            rows.push(c);
        }
        let mut parts = line.split_whitespace();
        let (Some(repo_name), Some(version)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some((repo, name)) = repo_name.split_once('/') else {
            continue;
        };
        let mut installed_version = None;
        for marker in parts {
            if let Some(inner) = marker
                .trim_matches(|c| c == '[' || c == ']')
                .strip_prefix("installed")
            {
                installed_version = Some(
                    inner
                        .strip_prefix(':')
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                );
                if installed_version.as_deref() == Some("") {
                    installed_version = Some(version.to_string());
                }
            }
        }
        current = Some(SyncSearchRow {
            repo: repo.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            installed_version,
            description: String::new(),
        });
    }
    if let Some(c) = current.take() {
        rows.push(c);
    }
    rows
}

/// A parsed `pacman -Si/-Qi` key/value block.
///
/// Values keep their per-line strings; multi-value fields are one entry per
/// line (continuation lines included).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InfoBlock {
    fields: Vec<(String, Vec<String>)>,
}

impl InfoBlock {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.first())
            .map(|s| s.as_str())
    }

    pub fn list(&self, key: &str) -> Vec<String> {
        let vals = self
            .fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        // Multiple values may share one line separated by two spaces.
        vals.into_iter()
            .flat_map(|line| {
                line.split("  ")
                    .map(str::trim)
                    .filter(|v| !v.is_empty() && *v != "None")
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn push(&mut self, key: String, value: String) {
        if let Some(entry) = self.fields.iter_mut().find(|(k, _)| *k == key) {
            entry.1.push(value);
        } else {
            self.fields.push((key, vec![value]));
        }
    }
}

/// Parses zero or more blank-line-separated `-Si` / `-Qi` blocks.
pub fn parse_info_blocks(text: &str) -> Vec<InfoBlock> {
    let mut blocks = Vec::new();
    let mut current = InfoBlock::default();

    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            if !current.fields.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            // Continuation line: pacman lists one value per indented line.
            if let Some((_, vals)) = current.fields.last_mut() {
                vals.push(line.trim().to_string());
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            // Skip group headers like "(1/2) ..." style noise defensively.
            if key.is_empty() {
                continue;
            }
            current.push(key, value);
        }
    }
    if !current.fields.is_empty() {
        blocks.push(current);
    }
    blocks
}

/// One row of `pacman -Qu --color never`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuRow {
    pub name: String,
    pub current: String,
    pub new: String,
    pub repo: Option<String>,
}

pub fn parse_qu_output(text: &str) -> Vec<QuRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        // Update rows always contain " -> "; ignore warnings/noise.
        if !line.contains("->") || line.starts_with("warning:") {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let (Some(name), Some(cur), Some(arrow), Some(new)) =
            (tokens.next(), tokens.next(), tokens.next(), tokens.next())
        else {
            continue;
        };
        if arrow != "->" {
            continue;
        }
        let repo = tokens
            .next()
            .map(|r| r.trim_matches(|c| c == '(' || c == ')').to_string());
        rows.push(QuRow {
            name: name.to_string(),
            current: cur.to_string(),
            new: new.to_string(),
            repo,
        });
    }
    rows
}

/// One row of `pacman -Sp --print-format '%r\t%n\t%v'`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRow {
    pub repo: String,
    pub name: String,
    pub version: String,
}

pub fn parse_plan_output(text: &str) -> Vec<PlanRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        match (cols.next(), cols.next(), cols.next()) {
            (Some(r), Some(n), Some(v)) if !r.contains(' ') => rows.push(PlanRow {
                repo: r.trim().to_string(),
                name: n.trim().to_string(),
                version: v.trim().to_string(),
            }),
            _ => continue,
        }
    }
    rows
}

/// Strips a version constraint from a dependency string.
/// `"gtk4>=1:4.16"` becomes `"gtk4"`; `"foo<2"` becomes `"foo"`.
pub fn strip_version_spec(dep: &str) -> &str {
    match dep.find(|c| c == '<' || c == '>' || c == '=') {
        Some(idx) => dep[..idx].trim(),
        None => dep.trim(),
    }
}

/// Splits an optional-dependency value into name and description.
/// `"hunspell: Spell checking"` → `("hunspell", Some("Spell checking"))`.
pub fn split_opt_dep(value: &str) -> (String, Option<String>) {
    match value.split_once(": ") {
        Some((name, desc)) => (name.trim().to_string(), Some(desc.trim().to_string())),
        None => (value.trim().to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH_FIXTURE: &str = "\
extra/firefox 133.0-2 [installed]
    Fast, Private & Secure Web Browser
core/linux 6.11.4.arch1-1 [base-devel]
    The Linux kernel and modules
extra/linux 6.11.4.arch1-1 [installed]
    The Linux kernel and modules
";

    #[test]
    fn parses_search_rows() {
        let rows = parse_search_output(SEARCH_FIXTURE);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0],
            SyncSearchRow {
                repo: "extra".into(),
                name: "firefox".into(),
                version: "133.0-2".into(),
                installed_version: Some("133.0-2".into()),
                description: "Fast, Private & Secure Web Browser".into(),
            }
        );
        assert_eq!(rows[1].installed_version, None);
        assert_eq!(rows[2].description, "The Linux kernel and modules");
    }

    #[test]
    fn search_installed_other_repo_marker() {
        let rows =
            parse_search_output("multilib/lib32-glibc 2.42-1 [installed: core]\n    GNU C\n");
        assert_eq!(rows[0].installed_version.as_deref(), Some("2.42-1"));
    }

    #[test]
    fn tolerates_garbage_lines() {
        let rows = parse_search_output("this is not pacman output\n\n\n");
        assert!(rows.is_empty());
    }

    const INFO_FIXTURE: &str = "\
Repository      : extra
Name            : firefox
Version         : 133.0-2
Description     : Fast web browser
Architecture    : x86_64
URL             : https://www.mozilla.org/firefox/
Licenses        : MPL-2.0
Groups          : None
Provides        : None
Depends On      : gtk4
                  gtk3
                  dbus-glib
Optional Deps   : hunspell: Spell checking
                  libnotify: Notification support
Required By     : None
Conflicts With  : None
Replaces        : None
Download Size   : 83.55 MiB
Installed Size  : 246.10 MiB
Packager        : Arch Linux <arch@example.org>
Build Date      : Tue 12 Nov 2024 10:00:00 UTC
Install Reason  : Explicitly installed
Install Script  : Yes
Validated By    : Signature

";

    #[test]
    fn parses_single_info_block() {
        let blocks = parse_info_blocks(INFO_FIXTURE);
        assert_eq!(blocks.len(), 1);
        let b = &blocks[0];
        assert_eq!(b.get("Name"), Some("firefox"));
        assert_eq!(b.get("Version"), Some("133.0-2"));
        assert_eq!(b.get("Download Size"), Some("83.55 MiB"));
        assert_eq!(b.get("URL"), Some("https://www.mozilla.org/firefox/"));
        let deps = b.list("Depends On");
        assert_eq!(deps, vec!["gtk4", "gtk3", "dbus-glib"]);
        let opt = b.list("Optional Deps");
        assert_eq!(opt.len(), 2);
        assert_eq!(
            split_opt_dep(&opt[0]),
            ("hunspell".into(), Some("Spell checking".into()))
        );
        assert!(b.list("Groups").is_empty(), "'None' is filtered out");
        assert_eq!(b.get("Install Reason"), Some("Explicitly installed"));
    }

    #[test]
    fn parses_multiple_blocks() {
        let two = format!(
            "{INFO_FIXTURE}Repository      : core\nName            : bash\nVersion         : 5.2-1\n"
        );
        let blocks = parse_info_blocks(&two);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1].get("Name"), Some("bash"));
    }

    #[test]
    fn parses_qu_rows() {
        let out = "\
firefox 133.0-2 -> 134.0-1 (extra)
linux-api-headers 6.9-1 -> 6.10-1 (any)
warning: ignoring package downgrade (x 1.0 => 0.9)
";
        let rows = parse_qu_output(out);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "firefox");
        assert_eq!(rows[0].current, "133.0-2");
        assert_eq!(rows[0].new, "134.0-1");
        assert_eq!(rows[0].repo.as_deref(), Some("extra"));
        assert_eq!(rows[1].repo.as_deref(), Some("any"));
    }

    #[test]
    fn parses_plan_rows() {
        let rows = parse_plan_output("extra\tfirefox\t133.0-2\nextra\tgtk4\t4.16.0-1\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1],
            PlanRow {
                repo: "extra".into(),
                name: "gtk4".into(),
                version: "4.16.0-1".into()
            }
        );
    }

    #[test]
    fn strips_version_specs() {
        assert_eq!(strip_version_spec("gtk4>=1:4.16"), "gtk4");
        assert_eq!(strip_version_spec("mesa<24"), "mesa");
        assert_eq!(strip_version_spec("bash"), "bash");
    }
}
