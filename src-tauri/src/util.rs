//! Paths, logging, hashing and time helpers shared by the other modules.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Roaming app data: settings, the user's favourites / edits / export history, the log.
pub fn data_dir() -> PathBuf {
    let d = dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("Backtrack");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Local cache: probe index, generated thumbnails, extracted audio, two-pass logs.
pub fn cache_dir() -> PathBuf {
    let d = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("Backtrack");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn cache_sub(name: &str) -> PathBuf {
    let d = cache_dir().join(name);
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn system_time_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Append a line to %APPDATA%\Backtrack\backtrack.log (and stderr in debug builds).
pub fn log(level: &str, msg: &str) {
    let line = format!("{} [{}] {}\n", now_ms(), level, msg);
    if cfg!(debug_assertions) {
        eprint!("{line}");
    }
    let path = data_dir().join("backtrack.log");
    // Keep the log from growing without bound.
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 2_000_000 {
            let _ = std::fs::rename(&path, data_dir().join("backtrack.old.log"));
        }
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Case-insensitive key for a path (Windows paths are case-insensitive).
pub fn path_key(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").to_lowercase()
}

/// Stable 64-bit FNV-1a hash rendered as hex; used for clip ids and cache file names.
pub fn fnv_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "other".into()
    } else {
        out
    }
}

/// Make a user-supplied file stem safe for Windows.
pub fn sanitize_stem(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if trimmed.is_empty() {
        "clip".into()
    } else {
        trimmed.chars().take(150).collect()
    }
}

/// `dir/stem.ext`, or `dir/stem (2).ext`, `(3)`... if taken.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 2..10_000 {
        let p = dir.join(format!("{stem} ({n}).{ext}"));
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{stem} ({}).{ext}", now_ms()))
}

/// Parse an ISO-8601 UTC timestamp like `2026-11-20T00:01:28Z` or `...28.150Z` to ms since epoch.
pub fn parse_iso_utc_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let num = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok());
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut ms = 0i64;
    if s.as_bytes().get(19) == Some(&b'.') {
        let frac: String = s[20..].chars().take_while(|c| c.is_ascii_digit()).collect();
        let f3: String = format!("{frac:0<3}").chars().take(3).collect();
        ms = f3.parse().unwrap_or(0);
    }
    // days_from_civil (Howard Hinnant)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + h * 3600 + mi * 60 + se;
    if secs < 0 {
        return None;
    }
    Some((secs * 1000 + ms) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn iso() {
        assert_eq!(parse_iso_utc_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_utc_ms("2026-11-20T00:01:28Z"), Some(1_795_132_888_000));
        assert_eq!(parse_iso_utc_ms("2026-11-20T01:55:10.150Z"), Some(1_795_139_710_150));
    }
    #[test]
    fn slug() {
        assert_eq!(slugify("Call of Duty®"), "call-of-duty");
        assert_eq!(slugify("Call of Duty: Modern Warfare 2 (2009)"), "call-of-duty-modern-warfare-2-2009");
    }
}
