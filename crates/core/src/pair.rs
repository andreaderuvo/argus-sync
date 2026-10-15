//! The folders this computer keeps in sync ("pairs"): which server, which key, where here.
//!
//! Kept in the user's config directory (`~/Library/Application Support/argus-sync` on a Mac,
//! `%APPDATA%\argus-sync` on Windows, `~/.config/argus-sync` on Linux), readable by the user only
//! where the system has such a thing. Each pair's state goes beside it, in its own file.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pair {
    pub id: String,
    /// What the person calls it (the folder's name on the server, by default).
    pub name: String,
    /// The Argus address, as the page was opened at.
    pub url: String,
    /// The sync key (`as_…`).
    pub key: String,
    /// Where it lives on this computer.
    pub local: PathBuf,
    /// Bytes above which a file waits to be asked for; 0 = no limit.
    #[serde(default = "default_big")]
    pub big: u64,
    #[serde(default)]
    pub paused: bool,
    /// Big files asked for, by key.
    #[serde(default)]
    pub wanted: BTreeSet<String>,
}

pub fn default_big() -> u64 {
    500 * 1024 * 1024
}

/// What a link from Argus carries: `argus-sync://pair?url=…&key=…&name=…`.
#[derive(Debug, Clone, PartialEq)]
pub struct Invite {
    pub url: String,
    pub key: String,
    pub name: String,
}

/// Read a link from Argus — or the same thing pasted, with or without its scheme.
pub fn parse_link(text: &str) -> Option<Invite> {
    let text = text.trim();
    let query = text.split_once('?').map(|(_, q)| q).unwrap_or(text);
    let mut url = None;
    let mut key = None;
    let mut name = String::new();
    for part in query.split('&') {
        let (k, v) = part.split_once('=')?;
        let v = urlencoding::decode(v).ok()?.into_owned();
        match k {
            "url" => url = Some(v),
            "key" => key = Some(v),
            "name" => name = v,
            _ => {}
        }
    }
    let url = url?.trim_end_matches('/').to_string();
    let key = key?;
    if !(url.starts_with("http://") || url.starts_with("https://")) || !key.starts_with("as_") {
        return None;
    }
    Some(Invite { url, key, name })
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("ARGUS_SYNC_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|d| d.join("argus-sync")))
        .unwrap_or_else(|| PathBuf::from(".argus-sync"))
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Pairs {
    #[serde(default)]
    pub pairs: Vec<Pair>,
}

impl Pairs {
    pub fn file(dir: &Path) -> PathBuf {
        dir.join("pairs.json")
    }

    pub fn load(dir: &Path) -> Pairs {
        fs::read(Self::file(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let tmp = dir.join("pairs.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(tmp, Self::file(dir))
    }

    pub fn add(&mut self, invite: &Invite, local: PathBuf) -> &Pair {
        let id = format!("{:012x}", rand::random::<u64>() & 0xffff_ffff_ffff);
        let name = if invite.name.is_empty() {
            local.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "argus".into())
        } else {
            invite.name.clone()
        };
        self.pairs.push(Pair { id, name, url: invite.url.clone(), key: invite.key.clone(), local, big: default_big(), paused: false, wanted: BTreeSet::new() });
        self.pairs.last().unwrap()
    }
}

pub fn state_file(dir: &Path, pair: &Pair) -> PathBuf {
    dir.join("state").join(format!("{}.json", pair.id))
}

pub fn fallback_trash(dir: &Path, pair: &Pair) -> PathBuf {
    dir.join("trash").join(&pair.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_from_argus_and_the_same_pasted() {
        let link = "argus-sync://pair?url=http%3A%2F%2Flab-server%3A8090&key=as_abc-DEF_123&name=report%20salmonella";
        let i = parse_link(link).unwrap();
        assert_eq!(i, Invite { url: "http://lab-server:8090".into(), key: "as_abc-DEF_123".into(), name: "report salmonella".into() });
        assert_eq!(parse_link(&format!("  {link}\n")), Some(i.clone()));
        assert_eq!(parse_link(link.split_once('?').unwrap().1), Some(i));
        assert!(parse_link("argus-sync://pair?url=ftp%3A%2F%2Fx&key=as_x").is_none());
        assert!(parse_link("argus-sync://pair?url=http%3A%2F%2Fx&key=not-a-sync-key").is_none());
        assert!(parse_link("hello").is_none());
    }
}
