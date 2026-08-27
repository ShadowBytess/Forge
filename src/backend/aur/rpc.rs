//! AUR RPC v5 client (structured JSON — the AUR's documented interface).
//!
//! <https://aur.archlinux.org/rpc/v5>

use serde::Deserialize;

use crate::error::{ForgeError, Result};

pub const AUR_BASE_URL: &str = "https://aur.archlinux.org";
/// The RPC caps info queries; stay safely under it.
const MAX_INFO_ARGS: usize = 200;

/// One package record from the RPC (search and info share most fields).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AurInfo {
    #[serde(rename = "ID")]
    pub id: u64,
    pub name: String,
    pub package_base: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(rename = "URLPath", default)]
    pub url_path: Option<String>,
    #[serde(default)]
    pub num_votes: u64,
    #[serde(default)]
    pub popularity: f64,
    /// Unix timestamp when flagged out-of-date, or `null`.
    #[serde(default)]
    pub out_of_date: Option<i64>,
    #[serde(default)]
    pub maintainer: Option<String>,
    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default)]
    pub make_depends: Vec<String>,
    #[serde(default)]
    pub check_depends: Vec<String>,
    #[serde(default)]
    pub opt_depends: Vec<String>,
    #[serde(default)]
    pub license: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub replaces: Vec<String>,
    #[serde(default)]
    pub last_modified: i64,
}

#[derive(Debug, Deserialize)]
struct RpcEnvelope {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    results: serde_json::Value,
    #[serde(default, alias = "Error")]
    error: Option<String>,
}

/// Thin async client over the RPC v5 API.
pub struct AurClient {
    http: reqwest::Client,
    base: String,
}

impl Default for AurClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AurClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "forge/",
                env!("CARGO_PKG_VERSION"),
                " (+aur rpc v5)"
            ))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base: AUR_BASE_URL.to_string(),
        }
    }

    async fn get(&self, path_and_query: String) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.base, path_and_query);
        let resp = self.http.get(&url).send().await?.error_for_status()?;
        let envelope: RpcEnvelope = resp.json().await?;
        if envelope.kind == "error" {
            return Err(ForgeError::Network(format!(
                "AUR RPC error: {}",
                envelope.error.unwrap_or_else(|| "unknown".into())
            )));
        }
        Ok(envelope.results)
    }

    /// `type=search&by=name-desc`
    pub async fn search(&self, query: &str) -> Result<Vec<AurInfo>> {
        let q = percent_encode(query.trim());
        let value = self.get(format!("/rpc/v5/search/{q}?by=name-desc")).await?;
        Ok(serde_json::from_value(value)?)
    }

    /// `type=info` for one or more packages.
    pub async fn info(&self, names: &[String]) -> Result<Vec<AurInfo>> {
        let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let mut out = Vec::new();
        for chunk in names.chunks(MAX_INFO_ARGS) {
            let mut pairs: Vec<(String, String)> =
                vec![("v".into(), "5".into()), ("type".into(), "info".into())];
            for n in chunk {
                pairs.push(("arg[]".into(), (*n).to_string()));
            }
            // Build the query string manually to keep arg[] keys intact.
            let qs = pairs
                .iter()
                .map(|(k, v)| format!("{k}={}", percent_encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            let value = self.get(format!("/rpc/v5/info?{qs}")).await?;
            let mut items: Vec<AurInfo> = serde_json::from_value(value)?;
            out.append(&mut items);
        }
        Ok(out)
    }
}

/// Minimal RFC 3986 unreserved-character encoder for URL components.
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_unreserved_only() {
        assert_eq!(percent_encode("firefox"), "firefox");
        assert_eq!(percent_encode("gtk 4"), "gtk%204");
        assert_eq!(percent_encode("a+b/c"), "a%2Bb%2Fc");
    }

    #[tokio::test]
    async fn parses_search_fixture() {
        let body = r#"{
            "version": 5,
            "type": "search",
            "resultcount": 1,
            "results": [{
                "ID": 61809, "Name": "yay", "PackageBase": "yay",
                "Version": "12.3.5-1",
                "Description": "Yet another yogurt. Pacman wrapper and AUR helper written in Go.",
                "URL": "https://github.com/Jguer/yay", "NumVotes": 3210,
                "Popularity": 42.7, "OutOfDate": null,
                "Maintainer": "Jguer", "FirstSubmitted": 1485912194,
                "LastModified": 1717000000, "URLPath": "/cgit/aur.git/snapshot/yay.tar.gz"
            }]
        }"#;
        let env: RpcEnvelope = serde_json::from_str(body).unwrap();
        assert_eq!(env.kind, "search");
        let items: Vec<AurInfo> = serde_json::from_value(env.results).unwrap();
        assert_eq!(items.len(), 1);
        let yay = &items[0];
        assert_eq!(yay.name, "yay");
        assert_eq!(yay.version, "12.3.5-1");
        assert_eq!(yay.num_votes, 3210);
        assert!(yay.out_of_date.is_none());
        assert_eq!(yay.maintainer.as_deref(), Some("Jguer"));
    }

    #[tokio::test]
    async fn parses_info_fields_with_defaults() {
        let body = r#"{"version":5,"type":"multiinfo","resultcount":1,"results":[{
            "ID": 1, "Name": "x", "PackageBase": "x", "Version": "1-1",
            "Depends": ["gtk4"], "MakeDepends": ["go>=1.22"], "OptDepends": ["noto-fonts: emoji"],
            "License": ["MIT"], "Conflicts": ["yay-git"]
        }]}"#;
        let items: Vec<AurInfo> =
            serde_json::from_value(serde_json::from_str::<RpcEnvelope>(body).unwrap().results)
                .unwrap();
        let x = &items[0];
        assert_eq!(x.depends, vec!["gtk4"]);
        assert_eq!(x.make_depends, vec!["go>=1.22"]);
        assert_eq!(x.conflicts, vec!["yay-git"]);
        assert!(x.url.is_none());
        assert_eq!(x.num_votes, 0);
    }

    #[tokio::test]
    async fn error_envelope_is_recognised() {
        let env: RpcEnvelope = serde_json::from_str(
            r#"{"version":5,"type":"error","resultcount":0,"results":[],"Error":"Incorrect request field specified"}"#,
        )
        .unwrap();
        assert_eq!(env.kind, "error");
        assert_eq!(
            env.error.as_deref(),
            Some("Incorrect request field specified")
        );
    }
}
