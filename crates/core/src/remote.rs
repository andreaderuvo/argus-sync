//! The Argus end: `/api/sync/*` with a sync key. Every path here is relative to the synced folder
//! and written with `/`, whatever the computer's separator.

use serde::Deserialize;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct Hello {
    pub name: String,
    pub folder: String,
    pub path: String,
    pub write: bool,
    pub host: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Entry {
    pub p: String,
    pub d: bool,
    pub s: u64,
    pub m: i64,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    same: bool,
    signature: String,
    #[serde(default)]
    entries: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
pub struct Written {
    pub s: u64,
    pub m: i64,
}

#[derive(Debug)]
pub enum RemoteError {
    /// What the client believed was there is not what is there: a 409, decided next round.
    Conflict(String),
    /// The key was taken back, or never was.
    Unauthorized,
    /// Refused, with the server's reason (read-only, outside the folder, …).
    Refused(u16, String),
    /// Could not reach the server.
    Network(String),
    Io(io::Error),
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::Conflict(s) => write!(f, "changed on the server meanwhile ({s})"),
            RemoteError::Unauthorized => write!(f, "the server no longer accepts this key — connect the folder again from Argus"),
            RemoteError::Refused(code, s) => write!(f, "the server refused ({code}): {s}"),
            RemoteError::Network(s) => write!(f, "cannot reach the server: {s}"),
            RemoteError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for RemoteError {
    fn from(e: io::Error) -> Self {
        RemoteError::Io(e)
    }
}

fn from_ureq(e: ureq::Error) -> RemoteError {
    match e {
        ureq::Error::Status(401, _) => RemoteError::Unauthorized,
        ureq::Error::Status(409, r) => RemoteError::Conflict(reason(r)),
        ureq::Error::Status(code, r) => RemoteError::Refused(code, reason(r)),
        ureq::Error::Transport(t) => RemoteError::Network(t.to_string()),
    }
}

fn reason(r: ureq::Response) -> String {
    let text = r.into_string().unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or(text)
}

/// `base`: what the client last saw at that path, so the server can refuse a write on top of a
/// change it has not seen.
#[derive(Debug, Clone, Copy)]
pub enum Base {
    /// Nothing should be there.
    Nothing,
    /// This modification time, in nanoseconds.
    Seen(i64),
    /// Write whatever is there (used only after a conflict was settled).
    Any,
}

impl Base {
    fn query(self) -> Option<String> {
        match self {
            Base::Nothing => Some("none".into()),
            Base::Seen(m) => Some(m.to_string()),
            Base::Any => None,
        }
    }
}

pub struct Remote {
    base: String,
    key: String,
    agent: ureq::Agent,
}

impl Remote {
    pub fn new(url: &str, key: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(120))
            .user_agent(concat!("argus-sync/", env!("CARGO_PKG_VERSION")))
            .build();
        Remote { base: url.trim_end_matches('/').to_string(), key: key.to_string(), agent }
    }

    fn url(&self, route: &str, query: &[(&str, String)]) -> String {
        let mut u = format!("{}/api/sync/{route}", self.base);
        for (i, (k, v)) in query.iter().enumerate() {
            u.push(if i == 0 { '?' } else { '&' });
            u.push_str(k);
            u.push('=');
            u.push_str(&urlencoding::encode(v));
        }
        u
    }

    fn auth(&self, req: ureq::Request) -> ureq::Request {
        req.set("authorization", &format!("Bearer {}", self.key))
    }

    pub fn hello(&self) -> Result<Hello, RemoteError> {
        let r = self.auth(self.agent.get(&self.url("hello", &[]))).call().map_err(from_ureq)?;
        r.into_json().map_err(RemoteError::Io)
    }

    /// Every entry, or `None` when nothing changed since `since` (the last signature).
    pub fn manifest(&self, since: Option<&str>) -> Result<(String, Option<Vec<Entry>>), RemoteError> {
        let q: Vec<(&str, String)> = since.map(|s| vec![("since", s.to_string())]).unwrap_or_default();
        let r = self.auth(self.agent.get(&self.url("manifest", &q))).call().map_err(from_ureq)?;
        let m: Manifest = r.into_json().map_err(RemoteError::Io)?;
        Ok((m.signature, if m.same { None } else { Some(m.entries) }))
    }

    /// The file, into `to` (created or truncated). Returns its size and its time on the server
    /// (ns) — the time of what was actually sent, which may be newer than the manifest said.
    pub fn download(&self, rel: &str, to: &Path) -> Result<(u64, i64), RemoteError> {
        let r = self.auth(self.agent.get(&self.url("file", &[("p", rel.to_string())]))).call().map_err(from_ureq)?;
        let m: i64 = r.header("x-mtime-ns").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut out = File::create(to)?;
        let n = io::copy(&mut r.into_reader(), &mut out)?;
        out.flush()?;
        out.sync_all()?;
        Ok((n, m))
    }

    pub fn upload(&self, rel: &str, from: &Path, mtime_ns: i64, base: Base) -> Result<Written, RemoteError> {
        let mut q = vec![("p", rel.to_string()), ("m", mtime_ns.to_string())];
        if let Some(b) = base.query() {
            q.push(("base", b));
        }
        let file = File::open(from)?;
        let len = file.metadata()?.len();
        let r = self
            .auth(self.agent.put(&self.url("file", &q)))
            .set("content-type", "application/octet-stream")
            .set("content-length", &len.to_string())
            .send(file.take(len))
            .map_err(from_ureq)?;
        r.into_json().map_err(RemoteError::Io)
    }

    pub fn delete(&self, rel: &str, base: Base) -> Result<(), RemoteError> {
        let mut q = vec![("p", rel.to_string())];
        if let Some(b) = base.query() {
            q.push(("base", b));
        }
        self.auth(self.agent.delete(&self.url("file", &q))).call().map_err(from_ureq)?;
        Ok(())
    }

    pub fn mkdir(&self, rel: &str) -> Result<(), RemoteError> {
        self.auth(self.agent.post(&self.url("dir", &[("p", rel.to_string())]))).call().map_err(from_ureq)?;
        Ok(())
    }

    pub fn rmdir(&self, rel: &str) -> Result<(), RemoteError> {
        self.auth(self.agent.delete(&self.url("dir", &[("p", rel.to_string())]))).call().map_err(from_ureq)?;
        Ok(())
    }
}
