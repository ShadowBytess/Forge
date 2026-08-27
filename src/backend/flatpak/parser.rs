//! Parsers for flatpak's column-based (`--columns=`) tab-separated output.

/// One row of `flatpak search --columns=...`.
///
/// `flatpak search` exposes a `remotes` column (not `origin`); we take the
/// first remote as the origin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchRow {
    pub app_id: String,
    pub name: String,
    pub version: String,
    pub branch: String,
    pub origin: String,
    pub description: String,
}

fn split_row(line: &str, expected: usize) -> Vec<String> {
    let cols: Vec<String> = line.split('\t').map(|c| c.trim().to_string()).collect();
    let mut cols = cols;
    cols.resize(expected.max(cols.len()), String::new());
    cols
}

const SEARCH_HEADER_PREFIXES: &[&str] = &["Name", "Description", "Application"];

pub fn parse_search_output(text: &str) -> Vec<SearchRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Skip the human header row ("Name  Description  Application ...").
        let first = line.split('\t').next().unwrap_or("").trim().to_string();
        if SEARCH_HEADER_PREFIXES.contains(&first.as_str()) && !first.contains('/') {
            continue;
        }
        let cols = split_row(line, 6);
        if cols[0].is_empty() || !cols[0].contains('.') {
            // Application ids always contain dots; anything else is noise.
            continue;
        }
        let origin = cols
            .get(4)
            .map(|r| r.split([',', ' ']).next().unwrap_or_default().to_string())
            .unwrap_or_default();
        rows.push(SearchRow {
            app_id: cols[0].clone(),
            name: cols[1].clone(),
            version: cols.get(2).cloned().unwrap_or_default(),
            branch: cols.get(3).cloned().unwrap_or_default(),
            origin,
            description: cols.get(5).cloned().unwrap_or_default(),
        });
    }
    rows
}

/// One row of `flatpak list --app --columns=...`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListRow {
    pub app_id: String,
    pub name: String,
    pub version: String,
    pub branch: String,
    pub origin: String,
    pub installed_size: Option<u64>,
    pub download_size: Option<u64>,
}

pub fn parse_list_output(text: &str) -> Vec<ListRow> {
    const HEADER_FIRSTS: &[&str] = &["Application", "Name"];
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let first = line.split('\t').next().unwrap_or("").trim().to_string();
        if HEADER_FIRSTS.contains(&first.as_str()) {
            continue;
        }
        let cols = split_row(line, 7);
        if cols[0].is_empty() {
            continue;
        }
        rows.push(ListRow {
            app_id: cols[0].clone(),
            name: cols[1].clone(),
            version: cols.get(2).cloned().unwrap_or_default(),
            branch: cols.get(3).cloned().unwrap_or_default(),
            origin: cols.get(4).cloned().unwrap_or_default(),
            installed_size: crate::util::size::parse_size(&cols[5]).ok(),
            download_size: crate::util::size::parse_size(&cols[6]).ok(),
        });
    }
    rows
}

/// One row of `flatpak remote-ls --updates --app --columns=...`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteUpdateRow {
    pub app_id: String,
    pub version: String,
    pub branch: String,
    pub origin: String,
}

pub fn parse_remote_ls_output(text: &str) -> Vec<RemoteUpdateRow> {
    const HEADER_FIRSTS: &[&str] = &["Application", "Name", "ID"];
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let first = line.split('\t').next().unwrap_or("").trim().to_string();
        if HEADER_FIRSTS.contains(&first.as_str()) {
            continue;
        }
        let cols = split_row(line, 4);
        if cols[0].is_empty() {
            continue;
        }
        rows.push(RemoteUpdateRow {
            app_id: cols[0].clone(),
            version: cols.get(1).cloned().unwrap_or_default(),
            branch: cols.get(2).cloned().unwrap_or_default(),
            origin: cols.get(3).cloned().unwrap_or_default(),
        });
    }
    rows
}

/// Key/value pairs from `flatpak info`'s aligned block.
#[derive(Clone, Debug, Default)]
pub struct InfoBlock {
    pub fields: std::collections::HashMap<String, String>,
}

impl InfoBlock {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(|s| s.as_str())
    }
}

pub fn parse_info_output(text: &str) -> InfoBlock {
    let mut block = InfoBlock::default();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            if key.chars().any(|c| c.is_whitespace()) || key.is_empty() || key.len() > 30 {
                continue;
            }
            block
                .fields
                .insert(key.to_string(), value.trim().to_string());
        }
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_rows_and_skips_header() {
        let text = "\
Application\tName\tVersion\tBranch\tOrigin\tDescription
org.mozilla.firefox\tFirefox\t128.0\tstable\tflathub\tSecure web browser
org.videolan.VLC\tVLC\t3.0.21\tstable\tflathub\tPlays everything
";
        let rows = parse_search_output(text);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].app_id, "org.mozilla.firefox");
        assert_eq!(rows[0].origin, "flathub");
        assert_eq!(rows[1].name, "VLC");
    }

    #[test]
    fn parses_list_rows_with_sizes() {
        let text = "\
Application	Name	Version	Branch	Origin	Installed size	Download size
org.mozilla.firefox	Firefox	128.0	stable	flathub	250 MB	83 MB
org.gimp.GIMP	GIMP	2.10.38	stable	flathub	400 MB	120 MB
";
        let rows = parse_list_output(text);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].installed_size, Some(250_000_000));
        assert_eq!(rows[0].download_size, Some(83_000_000));
        assert_eq!(rows[1].version, "2.10.38");
    }

    #[test]
    fn parses_remote_updates() {
        let text = "\
	Commit	Subject
org.chromium.Chromium	abc123	Update to 130.0	stable	flathub
";
        let _ = text;
        let text2 = "\
Application	Version	Branch	Origin
org.chromium.Chromium	130.0	stable	flathub
";
        let rows = parse_remote_ls_output(text2);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].version, "130.0");
    }

    #[test]
    fn parses_info_block() {
        let text = "\
         ID: org.mozilla.firefox
        Ref: app/org.mozilla.firefox/x86_64/stable
       Arch: x86_64
     Branch: stable
    Version: 128.0.3
 Installation: system
   Installed: 512.4 MB
     Origin: flathub
    Runtime: org.freedesktop.Platform/x86_64/23.08
";
        let block = parse_info_output(text);
        assert_eq!(block.get("ID"), Some("org.mozilla.firefox"));
        assert_eq!(block.get("Version"), Some("128.0.3"));
        assert_eq!(block.get("Installation"), Some("system"));
        assert_eq!(block.get("Origin"), Some("flathub"));
    }

    #[test]
    fn tolerates_garbage() {
        assert!(parse_search_output("random text\nno tabs here").is_empty());
        assert!(parse_list_output("").is_empty());
    }
}
