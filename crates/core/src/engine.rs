//! Carrying a plan out. Each step looks again right before it acts — a file being saved while a
//! download lands is the normal case, not a rare one — and leaves it for the next round when the
//! world moved under it, rather than overwrite what it has not seen.

use crate::local::{self, LocalEntry};
use crate::names;
use crate::plan::{self, Action, Issue, Options};
use crate::remote::{Base, Entry, Remote, RemoteError};
use crate::state::{Known, State};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Downloaded { path: String, bytes: u64 },
    Uploaded { path: String, bytes: u64 },
    DeletedHere { path: String },
    DeletedThere { path: String },
    FolderHere { path: String },
    FolderThere { path: String },
    /// Both changed and differed: both kept, the computer's under `copy`.
    Conflict { path: String, copy: String },
    /// Left for the next round, and why (a file open in another program, a change meanwhile…).
    Later { path: String, why: String },
}

#[derive(Debug, Default)]
pub struct Report {
    pub events: Vec<Event>,
    pub issues: Vec<Issue>,
    /// Nothing changed anywhere.
    pub idle: bool,
}

pub struct Settings {
    pub root: PathBuf,
    pub state_file: PathBuf,
    /// Where deletes go when the computer has no trash (a server without a desktop).
    pub fallback_trash: PathBuf,
    pub big: u64,
    pub wanted: BTreeSet<String>,
    /// The name conflict copies carry: "report (conflict from <host> …)".
    pub host: String,
}

pub struct Engine {
    pub remote: Remote,
    pub settings: Settings,
    pub state: State,
    cache: Option<Vec<Entry>>,
    case_insensitive: Option<bool>,
}

impl Engine {
    pub fn new(remote: Remote, settings: Settings) -> Engine {
        let state = State::load(&settings.state_file);
        Engine { remote, settings, state, cache: None, case_insensitive: None }
    }

    /// One round: look at both sides, do what they need, remember the result.
    pub fn sync_once(&mut self, on_event: &mut dyn FnMut(&Event)) -> Result<Report, RemoteError> {
        let hello = self.remote.hello()?;
        let since = if self.cache.is_some() { self.state.signature.clone() } else { None };
        let (signature, fresh) = self.remote.manifest(since.as_deref())?;
        if let Some(entries) = fresh {
            self.cache = Some(entries);
        }
        fs::create_dir_all(&self.settings.root)?;
        let ci = *self.case_insensitive.get_or_insert_with(|| local::case_insensitive(&self.settings.root));
        let here = local::scan(&self.settings.root)?;
        let entries = self.cache.clone().unwrap_or_default();
        let plan = plan::plan(&entries, &here, &self.state, &Options {
            case_insensitive: ci,
            windows_names: cfg!(windows),
            big: self.settings.big,
            wanted: &self.settings.wanted,
            can_write: hello.write,
        });
        let mut report = Report { issues: plan.issues, idle: plan.actions.is_empty(), events: vec![] };
        let mut touched_remote = false;
        for action in plan.actions {
            if matches!(action, Action::Upload { .. } | Action::DeleteRemote { .. } | Action::MkdirRemote { .. } | Action::RmdirRemote { .. } | Action::Conflict { .. }) {
                touched_remote = true;
            }
            match self.carry_out(action, &here, hello.write) {
                Ok(Some(ev)) => {
                    on_event(&ev);
                    report.events.push(ev);
                }
                Ok(None) => {}
                Err(RemoteError::Conflict(why)) | Err(RemoteError::Refused(_, why)) => {
                    // Decided again on the next round, from what is there then.
                    let ev = Event::Later { path: String::new(), why };
                    on_event(&ev);
                    report.events.push(ev);
                }
                Err(e @ (RemoteError::Unauthorized | RemoteError::Network(_))) => {
                    self.state.save(&self.settings.state_file)?;
                    return Err(e);
                }
                Err(RemoteError::Io(e)) => {
                    let ev = Event::Later { path: String::new(), why: e.to_string() };
                    on_event(&ev);
                    report.events.push(ev);
                }
            }
        }
        // What we just wrote changed the server's listing: ask for it whole next time.
        self.state.signature = if touched_remote { None } else { Some(signature) };
        if touched_remote {
            self.cache = None;
        }
        self.state.save(&self.settings.state_file)?;
        Ok(report)
    }

    fn path(&self, rel: &str) -> PathBuf {
        local::join(&self.settings.root, rel)
    }

    /// The computer's file is still what the plan saw (or, unknown to the plan, absent).
    fn unchanged_here(&self, rel: &str, seen: Option<&LocalEntry>) -> bool {
        let now = fs::symlink_metadata(self.path(rel)).ok();
        match (seen, now) {
            (None, None) => true,
            (Some(s), Some(m)) => !m.is_dir() && m.len() == s.size && local::mtime_ns(&m) == s.mtime,
            _ => false,
        }
    }

    fn remember(&mut self, key: &str, remote: &str, local_rel: &str, rs: u64, rm: i64) -> std::io::Result<()> {
        let m = fs::metadata(self.path(local_rel))?;
        self.state.files.insert(key.to_string(), Known {
            remote: remote.into(), local: local_rel.into(), dir: false, rs, rm, ls: m.len(), lm: local::mtime_ns(&m),
        });
        Ok(())
    }

    fn remember_dir(&mut self, key: &str, remote: &str, local_rel: &str) {
        self.state.files.insert(key.to_string(), Known { remote: remote.into(), local: local_rel.into(), dir: true, rs: 0, rm: 0, ls: 0, lm: 0 });
    }

    /// Download `remote` beside `local_rel` and return the part file, its size and its time.
    fn fetch(&self, remote: &str, local_rel: &str) -> Result<(PathBuf, u64, i64), RemoteError> {
        let target = self.path(local_rel);
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)?;
        }
        let part = local::part_beside(&target);
        match self.remote.download(remote, &part) {
            Ok((n, m)) => Ok((part, n, m)),
            Err(e) => {
                let _ = fs::remove_file(&part);
                Err(e)
            }
        }
    }

    fn carry_out(&mut self, action: Action, here: &BTreeMap<String, LocalEntry>, can_write: bool) -> Result<Option<Event>, RemoteError> {
        match action {
            Action::MkdirLocal { key, rel } => {
                fs::create_dir_all(self.path(&rel))?;
                self.remember_dir(&key, &rel, &rel);
                Ok(Some(Event::FolderHere { path: rel }))
            }
            Action::MkdirRemote { key, rel } => {
                self.remote.mkdir(&rel)?;
                self.remember_dir(&key, &rel, &rel);
                Ok(Some(Event::FolderThere { path: rel }))
            }
            Action::RememberDir { key, remote, local } => {
                self.remember_dir(&key, &remote, &local);
                Ok(None)
            }
            Action::Forget { key } => {
                self.state.files.remove(&key);
                Ok(None)
            }
            Action::Download { key, remote, local, .. } => {
                let (part, n, m) = self.fetch(&remote, &local)?;
                if !self.unchanged_here(&local, here.get(&key)) {
                    let _ = fs::remove_file(&part);
                    return Ok(Some(Event::Later { path: local, why: "changed on this computer meanwhile".into() }));
                }
                if let Err(e) = local::settle(&part, &self.path(&local), m) {
                    let _ = fs::remove_file(&part);
                    return Ok(Some(Event::Later { path: local, why: format!("could not be replaced ({e}) — open in another program?") }));
                }
                self.remember(&key, &remote, &local, n, m)?;
                Ok(Some(Event::Downloaded { path: local, bytes: n }))
            }
            Action::Upload { key, local, remote, base } => {
                let path = self.path(&local);
                let before = fs::metadata(&path)?;
                let written = self.remote.upload(&remote, &path, local::mtime_ns(&before), base.map(Base::Seen).unwrap_or(Base::Nothing))?;
                self.state.files.insert(key, Known {
                    remote: remote.clone(), local: local.clone(), dir: false, rs: written.s, rm: written.m, ls: before.len(), lm: local::mtime_ns(&before),
                });
                Ok(Some(Event::Uploaded { path: local, bytes: written.s }))
            }
            Action::DeleteLocal { key, local } => {
                let path = self.path(&local);
                if path.exists() {
                    if !self.unchanged_here(&local, here.get(&key)) {
                        return Ok(Some(Event::Later { path: local, why: "changed on this computer meanwhile".into() }));
                    }
                    local::to_trash(&path, &self.settings.fallback_trash)?;
                }
                self.state.files.remove(&key);
                Ok(Some(Event::DeletedHere { path: local }))
            }
            Action::DeleteRemote { key, remote, base } => {
                match self.remote.delete(&remote, Base::Seen(base)) {
                    Ok(()) | Err(RemoteError::Refused(404, _)) => {}
                    Err(e) => return Err(e),
                }
                self.state.files.remove(&key);
                Ok(Some(Event::DeletedThere { path: remote }))
            }
            Action::RmdirLocal { key, local } => {
                let path = self.path(&local);
                // What the system leaves in a folder does not make it "not empty".
                if let Ok(items) = fs::read_dir(&path) {
                    for item in items.flatten() {
                        if names::ignored(&item.file_name().to_string_lossy()) {
                            let _ = if item.path().is_dir() { fs::remove_dir_all(item.path()) } else { fs::remove_file(item.path()) };
                        }
                    }
                }
                match fs::remove_dir(&path) {
                    Ok(()) => {
                        self.state.files.remove(&key);
                        Ok(Some(Event::DeletedHere { path: local }))
                    }
                    Err(_) if !path.exists() => {
                        self.state.files.remove(&key);
                        Ok(None)
                    }
                    Err(_) => Ok(None),                // something new in it: it stays
                }
            }
            Action::RmdirRemote { key, remote } => match self.remote.rmdir(&remote) {
                Ok(()) | Err(RemoteError::Refused(404, _)) => {
                    self.state.files.remove(&key);
                    Ok(Some(Event::DeletedThere { path: remote }))
                }
                Err(RemoteError::Conflict(_)) => Ok(None),   // something new in it: it stays
                Err(e) => Err(e),
            },
            Action::Conflict { key, remote, local, size, .. } => {
                if self.settings.big > 0 && size > self.settings.big && !self.settings.wanted.contains(&key) {
                    return Ok(Some(Event::Later { path: local, why: "differs from the server's, which is too big to fetch without asking".into() }));
                }
                let (part, n, m) = self.fetch(&remote, &local)?;
                let path = self.path(&local);
                if local::same_content(&part, &path)? {
                    let _ = fs::remove_file(&part);
                    self.remember(&key, &remote, &local, n, m)?;
                    return Ok(None);
                }
                if !self.unchanged_here(&local, here.get(&key)) {
                    let _ = fs::remove_file(&part);
                    return Ok(Some(Event::Later { path: local, why: "changed on this computer meanwhile".into() }));
                }
                // Keep both: the computer's copy steps aside under a name that says whose it is.
                let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let when = chrono::Local::now().format("%Y-%m-%d %H%M").to_string();
                let copy_name = names::conflict_name(&name, &self.settings.host, &when);
                let copy_rel = match local.rfind('/') {
                    Some(i) => format!("{}/{copy_name}", &local[..i]),
                    None => copy_name.clone(),
                };
                fs::rename(&path, self.path(&copy_rel))?;
                if let Err(e) = local::settle(&part, &path, m) {
                    let _ = fs::rename(self.path(&copy_rel), &path);     // put it back as it was
                    let _ = fs::remove_file(&part);
                    return Ok(Some(Event::Later { path: local, why: format!("could not be replaced ({e})") }));
                }
                self.remember(&key, &remote, &local, n, m)?;
                if can_write {
                    let copy_path = self.path(&copy_rel);
                    let meta = fs::metadata(&copy_path)?;
                    let w = self.remote.upload(&copy_rel, &copy_path, local::mtime_ns(&meta), Base::Nothing)?;
                    self.state.files.insert(names::key(&copy_rel), Known {
                        remote: copy_rel.clone(), local: copy_rel.clone(), dir: false, rs: w.s, rm: w.m, ls: meta.len(), lm: local::mtime_ns(&meta),
                    });
                }
                Ok(Some(Event::Conflict { path: local, copy: copy_rel }))
            }
        }
    }
}

/// This computer's name, for conflict copies.
pub fn host_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()))
        .or_else(|| std::process::Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()))
        .map(|s| s.split('.').next().unwrap_or("").to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this computer".into())
}

#[allow(dead_code)]
fn _assert_path(_: &Path) {}
