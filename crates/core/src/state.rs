//! What both sides had when they last agreed, per file: the only memory two-way sync needs.
//! Kept as JSON beside the other pairs' state, written whole to a temporary file and renamed.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Known {
    /// The name on the server, and on the computer — the same file, maybe in two Unicode forms.
    pub remote: String,
    pub local: String,
    #[serde(default)]
    pub dir: bool,
    /// Size and modification time (ns) on the server…
    pub rs: u64,
    pub rm: i64,
    /// …and on the computer, as each side reported them. Times are never compared across sides:
    /// their file systems keep different precisions.
    pub ls: u64,
    pub lm: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub files: BTreeMap<String, Known>,
}

impl State {
    pub fn load(path: &Path) -> State {
        fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec(self)?)?;
        fs::rename(tmp, path)
    }
}
