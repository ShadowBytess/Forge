//! Parsing and formatting of human-readable byte sizes as printed by
//! pacman ("83.55 MiB") and flatpak ("512.3 MB").

use crate::error::{ForgeError, Result};

/// Parses strings like `"83.55 MiB"`, `"1.2 GB"` or `"4096 B"` into bytes.
///
/// Units containing `i` (KiB, MiB, GiB, TiB) are binary multiples; everything
/// else (KB, MB, GB) is treated as decimal, matching flatpak's output.
pub fn parse_size(text: &str) -> Result<u64> {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|c| !c.is_ascii_control())
        .collect();
    let mut split = cleaned.splitn(2, char::is_whitespace);
    let number = split.next().unwrap_or_default().trim();
    let unit = split.next().unwrap_or("B").trim();

    let value: f64 = number.parse().map_err(|_| ForgeError::Parse {
        context: "size",
        detail: format!("bad number in {text:?}"),
    })?;

    let bytes = match unit.to_ascii_lowercase().as_str() {
        "b" | "" => value,
        "kb" | "k" => value * 1_000.0,
        "mb" | "m" => value * 1_000_000.0,
        "gb" | "g" => value * 1_000_000_000.0,
        "tb" | "t" => value * 1_000_000_000_000.0,
        "kib" => value * 1024.0,
        "mib" => value * 1024.0 * 1024.0,
        "gib" => value * 1024.0 * 1024.0 * 1024.0,
        "tib" => value * 1024.0 * 1024.0 * 1024.0 * 1024.0,
        other => {
            return Err(ForgeError::Parse {
                context: "size",
                detail: format!("unknown unit {other:?} in {text:?}"),
            });
        }
    };
    Ok(bytes.round() as u64)
}

/// Formats a byte count for display, e.g. `83.6 MiB`.
pub fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KIB {
        format!("{bytes} B")
    } else if b < KIB * KIB {
        format!("{:.1} KiB", b / KIB)
    } else if b < KIB * KIB * KIB {
        format!("{:.1} MiB", b / (KIB * KIB))
    } else {
        format!("{:.2} GiB", b / (KIB * KIB * KIB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pacman_sizes() {
        let expected = (83.55_f64 * 1024.0 * 1024.0).round() as u64;
        assert_eq!(parse_size("83.55 MiB").unwrap(), expected);
        assert_eq!(parse_size("12 KiB").unwrap(), 12 * 1024);
        assert_eq!(parse_size("3 GiB").unwrap(), 3 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("512 B").unwrap(), 512);
    }

    #[test]
    fn parses_flatpak_sizes() {
        assert_eq!(parse_size("1.2 MB").unwrap(), 1_200_000);
        assert_eq!(parse_size("500 kB").unwrap(), 500_000);
        assert_eq!(parse_size("2 GB").unwrap(), 2_000_000_000);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_size("huge").is_err());
        assert!(parse_size("").is_err());
        assert!(parse_size("12 Zork").is_err());
    }

    #[test]
    fn formats_for_display() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2.0 KiB");
        assert_eq!(format_size((83.55 * 1024.0 * 1024.0) as u64), "83.5 MiB");
        assert_eq!(
            format_size(3 * 1024 * 1024 * 1024 + 100 * 1024 * 1024),
            "3.10 GiB"
        );
    }
}
