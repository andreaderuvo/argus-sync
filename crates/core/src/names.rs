//! File names across operating systems.
//!
//! The server is Linux: case-sensitive, any byte but `/` and NUL in a name, names stored in
//! whatever Unicode form they were written in. The computer may be a Mac (case-insensitive by
//! default, names often in NFD), Windows (case-insensitive, a list of characters and names it
//! refuses) or Linux. Every rule about "is this the same file" and "can this file exist here"
//! lives in this module, so it is tested once and used everywhere.

use unicode_normalization::UnicodeNormalization;

/// The key two names are compared by: Unicode NFC, so `è` written by a Mac (e + combining grave)
/// and `è` written by Linux (one code point) are the same file.
pub fn key(rel: &str) -> String {
    rel.nfc().collect()
}

/// The key two names collide by on a case-insensitive file system (macOS, Windows).
pub fn fold(rel: &str) -> String {
    key(rel).to_lowercase()
}

/// Left alone on both sides: what operating systems and editors leave behind on their own, and
/// argus-sync's own working files.
pub fn ignored(name: &str) -> bool {
    const EXACT: &[&str] = &[
        ".DS_Store", "Thumbs.db", "ehthumbs.db", "desktop.ini", "Desktop.ini", ".argus-sync",
        "$RECYCLE.BIN", ".Trash", ".Trashes", ".fseventsd", ".Spotlight-V100", ".TemporaryItems",
        ".DocumentRevisions-V100", "Icon\r", ".directory",
    ];
    EXACT.contains(&name)
        || name.starts_with(".argus-sync-")                       // a transfer in progress
        || name.starts_with("~$")                                 // Microsoft Office's lock file
        || (name.starts_with(".~lock.") && name.ends_with('#'))   // LibreOffice's
        || name.starts_with("._")                                 // macOS resource forks on foreign disks
        || name.ends_with(".tmp") && name.starts_with('~')
}

/// Why Windows cannot hold a file of this name, if it cannot.
pub fn windows_problem(name: &str) -> Option<&'static str> {
    if name.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*') || (c as u32) < 32) {
        return Some("a character Windows does not allow in a name (< > : \" \\ | ? * or a control character)");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Some("a name ending with a dot or a space, which Windows does not keep");
    }
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL"];
    let numbered = (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && stem.as_bytes()[3].is_ascii_digit()
        && stem.as_bytes()[3] != b'0';
    if RESERVED.contains(&stem.as_str()) || numbered {
        return Some("a name Windows reserves for a device (CON, PRN, AUX, NUL, COM1…, LPT1…)");
    }
    None
}

/// Why this computer cannot hold `rel`, if it cannot: checked part by part.
pub fn local_problem(rel: &str, windows: bool) -> Option<&'static str> {
    if !windows {
        return None;
    }
    rel.split('/').find_map(windows_problem)
}

/// The name a conflicting copy gets: `report (conflict from laptop 2026-10-14 1532).pdf`.
pub fn conflict_name(name: &str, host: &str, when: &str) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let host: String = host.chars().filter(|c| c.is_alphanumeric() || *c == '-').collect();
    format!("{stem} (conflict from {host} {when}){ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mac_name_and_a_linux_name_are_the_same_file() {
        let nfd = "relazione_e\u{0300}.pdf";
        let nfc = "relazione_\u{00e8}.pdf";
        assert_ne!(nfd, nfc);
        assert_eq!(key(nfd), key(nfc));
        assert_eq!(fold("Report.PDF"), fold("report.pdf"));
    }

    #[test]
    fn what_windows_refuses() {
        for bad in ["a:b.txt", "what?.txt", "x*.csv", "end.", "end ", "CON", "con.txt", "Com1.log", "lpt9", "tab\there"] {
            assert!(windows_problem(bad).is_some(), "{bad}");
        }
        for fine in ["report.pdf", "COM10.txt", "console.log", "COM0", ".hidden", "è.txt", "a b.txt"] {
            assert!(windows_problem(fine).is_none(), "{fine}");
        }
        assert!(local_problem("ok/bad:name/x.txt", true).is_some());
        assert!(local_problem("ok/bad:name/x.txt", false).is_none());
    }

    #[test]
    fn litter_is_ignored() {
        for name in [".DS_Store", "Thumbs.db", "desktop.ini", "~$report.docx", ".~lock.data.ods#", "._photo.jpg", ".argus-sync-3f.part"] {
            assert!(ignored(name), "{name}");
        }
        for name in ["report.docx", ".gitignore", "data.ods", "lock.txt"] {
            assert!(!ignored(name), "{name}");
        }
    }

    #[test]
    fn a_conflict_copy_keeps_its_extension() {
        assert_eq!(conflict_name("report.pdf", "lap 684", "2026-10-14 1532"), "report (conflict from lap684 2026-10-14 1532).pdf");
        assert_eq!(conflict_name("Makefile", "h", "d"), "Makefile (conflict from h d)");
        assert_eq!(conflict_name(".env", "h", "d"), ".env (conflict from h d)");
    }
}
