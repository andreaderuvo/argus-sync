//! What to do, decided from three things only: what the server has, what the computer has, and
//! what both had when they last agreed (the state). Pure — no network, no disk — so every case
//! below is a test.
//!
//! The rule is the one every two-way sync uses: a side *changed* when it differs from the state.
//! Changed on one side → carry it to the other. Changed on both → a conflict, where nothing is
//! lost (both copies are kept). Never in the state and on one side only → it is new there.

use crate::local::LocalEntry;
use crate::names;
use crate::remote::Entry;
use crate::state::{Known, State};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    MkdirLocal { key: String, rel: String },
    MkdirRemote { key: String, rel: String },
    Download { key: String, remote: String, local: String, size: u64, mtime: i64 },
    Upload { key: String, local: String, remote: String, base: Option<i64> },
    DeleteLocal { key: String, local: String },
    DeleteRemote { key: String, remote: String, base: i64 },
    /// Both sides changed (or both appeared): compare, and keep both when they differ.
    Conflict { key: String, remote: String, local: String, size: u64, mtime: i64 },
    /// Gone from both sides: nothing to do but forget it.
    Forget { key: String },
    /// A folder on both sides that was not known yet: remember it.
    RememberDir { key: String, remote: String, local: String },
    RmdirLocal { key: String, local: String },
    RmdirRemote { key: String, remote: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Issue {
    /// The server has a name this computer cannot hold.
    BadName { remote: String, why: &'static str },
    /// Two names on the server that differ only by case; this computer can hold one of them.
    CaseClash { kept: String, skipped: String },
    /// A file on one side, a folder on the other.
    FileVersusFolder { path: String },
    /// Over the size limit, waiting to be asked for.
    Big { remote: String, size: u64 },
}

#[derive(Debug, Default)]
pub struct Plan {
    pub actions: Vec<Action>,
    pub issues: Vec<Issue>,
}

pub struct Options<'a> {
    /// The computer's file system cannot tell `A` from `a`.
    pub case_insensitive: bool,
    /// The computer is Windows: refuse what Windows refuses.
    pub windows_names: bool,
    /// Files larger than this (bytes) wait to be asked for; 0 = no limit.
    pub big: u64,
    /// Big files the person asked for, by key.
    pub wanted: &'a BTreeSet<String>,
    /// The server may be written to.
    pub can_write: bool,
}

fn depth(rel: &str) -> usize {
    rel.matches('/').count()
}

pub fn plan(remote: &[Entry], local: &BTreeMap<String, LocalEntry>, state: &State, opt: &Options) -> Plan {
    let mut out = Plan::default();

    // The server's entries as this computer can have them.
    let mut r: BTreeMap<String, &Entry> = BTreeMap::new();
    let mut folded: BTreeMap<String, String> = BTreeMap::new();
    for e in remote {
        if let Some(why) = names::local_problem(&e.p, opt.windows_names) {
            out.issues.push(Issue::BadName { remote: e.p.clone(), why });
            continue;
        }
        let key = names::key(&e.p);
        if opt.case_insensitive {
            let f = names::fold(&e.p);
            if let Some(first) = folded.get(&f) {
                // Keep the one already synced, else the first in order; say so for the other.
                let keep_new = state.files.contains_key(&key) && !state.files.contains_key(first);
                let (kept, skipped) = if keep_new { (key.clone(), first.clone()) } else { (first.clone(), key.clone()) };
                out.issues.push(Issue::CaseClash { kept: kept.clone(), skipped: skipped.clone() });
                if keep_new {
                    r.remove(first);
                    folded.insert(f, key.clone());
                    r.insert(key, e);
                }
                continue;
            }
            folded.insert(f, key.clone());
        }
        r.insert(key, e);
    }

    let keys: BTreeSet<&String> = r.keys().chain(local.keys()).chain(state.files.keys()).collect();
    let mut mk: Vec<Action> = Vec::new();
    let mut files: Vec<Action> = Vec::new();
    let mut rm: Vec<(usize, Action)> = Vec::new();

    for key in keys {
        let (re, le, known) = (r.get(key).copied(), local.get(key), state.files.get(key));
        let remote_name = |le: &LocalEntry| known.map(|k| k.remote.clone()).unwrap_or_else(|| le.real.clone());

        // A file on one side, a folder on the other: neither is touched.
        if let (Some(re), Some(le)) = (re, le) {
            if re.d != le.dir {
                out.issues.push(Issue::FileVersusFolder { path: re.p.clone() });
                continue;
            }
        }
        let is_dir = re.map(|e| e.d).or(le.map(|e| e.dir)).or(known.map(|k| k.dir)).unwrap_or(false);

        if is_dir {
            match (re, le, known) {
                (Some(re), None, None) => mk.push(Action::MkdirLocal { key: key.clone(), rel: re.p.clone() }),
                (None, Some(le), None) if opt.can_write => mk.push(Action::MkdirRemote { key: key.clone(), rel: le.real.clone() }),
                (Some(re), Some(le), None) => files.push(Action::RememberDir { key: key.clone(), remote: re.p.clone(), local: le.real.clone() }),
                (None, Some(le), Some(_)) if opt.can_write => rm.push((depth(&le.real), Action::RmdirLocal { key: key.clone(), local: le.real.clone() })),
                (Some(re), None, Some(_)) if opt.can_write => rm.push((depth(&re.p), Action::RmdirRemote { key: key.clone(), remote: re.p.clone() })),
                (Some(re), None, Some(_)) => mk.push(Action::MkdirLocal { key: key.clone(), rel: re.p.clone() }),
                (None, None, Some(_)) => files.push(Action::Forget { key: key.clone() }),
                _ => {}
            }
            continue;
        }

        let changed_remote = |k: &Known| match re {
            Some(e) => e.s != k.rs || e.m != k.rm,
            None => true,
        };
        let changed_local = |k: &Known| match le {
            Some(e) => e.size != k.ls || e.mtime != k.lm,
            None => true,
        };
        let download = |re: &Entry, local: String| {
            if opt.big > 0 && re.s > opt.big && !opt.wanted.contains(key) {
                None
            } else {
                Some(Action::Download { key: key.clone(), remote: re.p.clone(), local, size: re.s, mtime: re.m })
            }
        };
        let big = |re: &Entry| Issue::Big { remote: re.p.clone(), size: re.s };

        match known {
            None => match (re, le) {
                (Some(re), None) => match download(re, re.p.clone()) {
                    Some(a) => files.push(a),
                    None => out.issues.push(big(re)),
                },
                (None, Some(le)) if opt.can_write => files.push(Action::Upload { key: key.clone(), local: le.real.clone(), remote: le.real.clone(), base: None }),
                (Some(re), Some(le)) => files.push(Action::Conflict { key: key.clone(), remote: re.p.clone(), local: le.real.clone(), size: re.s, mtime: re.m }),
                _ => {}
            },
            Some(k) => {
                let (rc, lc) = (changed_remote(k), changed_local(k));
                match (rc, lc) {
                    (false, false) => {}
                    (true, false) => match re {
                        None => files.push(Action::DeleteLocal { key: key.clone(), local: le.map(|l| l.real.clone()).unwrap_or_else(|| k.local.clone()) }),
                        Some(re) => match download(re, le.map(|l| l.real.clone()).unwrap_or_else(|| k.local.clone())) {
                            Some(a) => files.push(a),
                            None => out.issues.push(big(re)),
                        },
                    },
                    (false, true) if opt.can_write => match le {
                        None => files.push(Action::DeleteRemote { key: key.clone(), remote: k.remote.clone(), base: k.rm }),
                        Some(le) => files.push(Action::Upload { key: key.clone(), local: le.real.clone(), remote: remote_name(le), base: Some(k.rm) }),
                    },
                    (false, true) => {
                        // Changed here, but the server takes nothing: the server's copy wins back.
                        if let Some(re) = re {
                            if let Some(a) = download(re, le.map(|l| l.real.clone()).unwrap_or_else(|| k.local.clone())) {
                                files.push(a);
                            }
                        }
                    }
                    (true, true) => match (re, le) {
                        (None, None) => files.push(Action::Forget { key: key.clone() }),
                        // Deleted there, changed here: the change wins (nothing is lost).
                        (None, Some(le)) if opt.can_write => files.push(Action::Upload { key: key.clone(), local: le.real.clone(), remote: remote_name(le), base: None }),
                        (None, Some(le)) => files.push(Action::DeleteLocal { key: key.clone(), local: le.real.clone() }),
                        // Deleted here, changed there: the change wins.
                        (Some(re), None) => match download(re, k.local.clone()) {
                            Some(a) => files.push(a),
                            None => out.issues.push(big(re)),
                        },
                        (Some(re), Some(le)) => files.push(Action::Conflict { key: key.clone(), remote: re.p.clone(), local: le.real.clone(), size: re.s, mtime: re.m }),
                    },
                }
            }
        }
    }

    mk.sort_by_key(|a| match a {
        Action::MkdirLocal { rel, .. } | Action::MkdirRemote { rel, .. } => depth(rel),
        _ => 0,
    });
    rm.sort_by_key(|(d, _)| std::cmp::Reverse(*d));
    out.actions.extend(mk);
    out.actions.extend(files);
    out.actions.extend(rm.into_iter().map(|(_, a)| a));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(p: &str, s: u64, m: i64) -> Entry {
        Entry { p: p.into(), d: false, s, m }
    }
    fn rd(p: &str) -> Entry {
        Entry { p: p.into(), d: true, s: 0, m: 0 }
    }
    fn l(real: &str, size: u64, mtime: i64) -> (String, LocalEntry) {
        (names::key(real), LocalEntry { real: real.into(), dir: false, size, mtime })
    }
    fn ld(real: &str) -> (String, LocalEntry) {
        (names::key(real), LocalEntry { real: real.into(), dir: true, size: 0, mtime: 0 })
    }
    fn known(p: &str, rs: u64, rm: i64, ls: u64, lm: i64) -> (String, Known) {
        (names::key(p), Known { remote: p.into(), local: p.into(), dir: false, rs, rm, ls, lm })
    }
    fn kd(p: &str) -> (String, Known) {
        (names::key(p), Known { remote: p.into(), local: p.into(), dir: true, rs: 0, rm: 0, ls: 0, lm: 0 })
    }
    fn go(remote: Vec<Entry>, local: Vec<(String, LocalEntry)>, state: Vec<(String, Known)>) -> Plan {
        go_with(remote, local, state, false, false, 0)
    }
    fn go_with(remote: Vec<Entry>, local: Vec<(String, LocalEntry)>, state: Vec<(String, Known)>, ci: bool, win: bool, big: u64) -> Plan {
        let wanted = BTreeSet::new();
        let st = State { files: state.into_iter().collect(), ..Default::default() };
        plan(&remote, &local.into_iter().collect(), &st, &Options { case_insensitive: ci, windows_names: win, big, wanted: &wanted, can_write: true })
    }
    fn kinds(p: &Plan) -> Vec<String> {
        p.actions.iter().map(|a| format!("{a:?}").split([' ', '{']).next().unwrap().to_string()).collect()
    }

    #[test]
    fn new_on_either_side_goes_to_the_other() {
        let p = go(vec![r("a.txt", 1, 10), rd("dir")], vec![l("b.txt", 2, 20), ld("here")], vec![]);
        assert_eq!(kinds(&p), ["MkdirLocal", "MkdirRemote", "Download", "Upload"]);
    }

    #[test]
    fn a_change_on_one_side_is_carried_with_what_it_was() {
        let p = go(vec![r("a.txt", 5, 99)], vec![l("a.txt", 1, 10)], vec![known("a.txt", 1, 10, 1, 10)]);
        assert!(matches!(&p.actions[0], Action::Download { mtime: 99, .. }));
        let p = go(vec![r("a.txt", 1, 10)], vec![l("a.txt", 7, 77)], vec![known("a.txt", 1, 10, 1, 10)]);
        assert!(matches!(&p.actions[0], Action::Upload { base: Some(10), .. }), "{p:?}");
        assert!(go(vec![r("a.txt", 1, 10)], vec![l("a.txt", 1, 10)], vec![known("a.txt", 1, 10, 1, 10)]).actions.is_empty());
    }

    #[test]
    fn deletes_follow_and_a_change_beats_a_delete() {
        assert_eq!(kinds(&go(vec![], vec![l("a.txt", 1, 10)], vec![known("a.txt", 1, 10, 1, 10)])), ["DeleteLocal"]);
        assert_eq!(kinds(&go(vec![r("a.txt", 1, 10)], vec![], vec![known("a.txt", 1, 10, 1, 10)])), ["DeleteRemote"]);
        // Deleted on the server, edited here: uploaded again, as new.
        let p = go(vec![], vec![l("a.txt", 3, 30)], vec![known("a.txt", 1, 10, 1, 10)]);
        assert!(matches!(&p.actions[0], Action::Upload { base: None, .. }));
        // Deleted here, edited on the server: downloaded again.
        assert_eq!(kinds(&go(vec![r("a.txt", 4, 40)], vec![], vec![known("a.txt", 1, 10, 1, 10)])), ["Download"]);
        assert_eq!(kinds(&go(vec![], vec![], vec![known("a.txt", 1, 10, 1, 10)])), ["Forget"]);
    }

    #[test]
    fn both_changed_or_both_new_is_a_conflict() {
        assert_eq!(kinds(&go(vec![r("a.txt", 4, 40)], vec![l("a.txt", 3, 30)], vec![known("a.txt", 1, 10, 1, 10)])), ["Conflict"]);
        assert_eq!(kinds(&go(vec![r("a.txt", 4, 40)], vec![l("a.txt", 4, 30)], vec![])), ["Conflict"], "first sync into a folder that has files");
    }

    #[test]
    fn a_mac_name_is_the_server_file_not_a_new_one() {
        let nfc = "relazione_\u{00e8}.pdf";
        let nfd = "relazione_e\u{0300}.pdf";
        let p = go(vec![r(nfc, 1, 10)], vec![l(nfd, 1, 10)], vec![(names::key(nfc), Known { remote: nfc.into(), local: nfd.into(), dir: false, rs: 1, rm: 10, ls: 1, lm: 10 })]);
        assert!(p.actions.is_empty(), "{p:?}");
        let p = go(vec![r(nfc, 1, 10)], vec![l(nfd, 2, 20)], vec![(names::key(nfc), Known { remote: nfc.into(), local: nfd.into(), dir: false, rs: 1, rm: 10, ls: 1, lm: 10 })]);
        assert!(matches!(&p.actions[0], Action::Upload { remote, .. } if remote == nfc), "uploaded under the server's own name: {p:?}");
    }

    #[test]
    fn two_names_that_differ_by_case_on_a_case_insensitive_disk() {
        let p = go_with(vec![r("Report.pdf", 1, 10), r("report.pdf", 2, 20)], vec![], vec![], true, false, 0);
        assert_eq!(kinds(&p), ["Download"]);
        assert!(matches!(&p.issues[0], Issue::CaseClash { .. }));
        let p = go_with(vec![r("Report.pdf", 1, 10), r("report.pdf", 2, 20)], vec![], vec![], false, false, 0);
        assert_eq!(kinds(&p), ["Download", "Download"], "Linux holds both");
    }

    #[test]
    fn names_windows_refuses_are_said_not_synced() {
        let p = go_with(vec![r("ok.txt", 1, 1), r("what?.txt", 1, 1), r("dir:x/a.txt", 1, 1)], vec![], vec![], true, true, 0);
        assert_eq!(kinds(&p), ["Download"]);
        assert_eq!(p.issues.len(), 2);
    }

    #[test]
    fn a_big_file_waits_to_be_asked_for() {
        let p = go_with(vec![r("reads.fastq.gz", 10_000_000_000, 1), r("small.txt", 10, 1)], vec![], vec![], false, false, 500_000_000);
        assert_eq!(kinds(&p), ["Download"]);
        assert!(matches!(&p.issues[0], Issue::Big { size: 10_000_000_000, .. }));
    }

    #[test]
    fn folders_are_made_parents_first_and_removed_children_first() {
        let p = go(vec![rd("a/b/c"), rd("a"), rd("a/b")], vec![], vec![]);
        let order: Vec<String> = p.actions.iter().map(|a| match a { Action::MkdirLocal { rel, .. } => rel.clone(), _ => String::new() }).collect();
        assert_eq!(order, ["a", "a/b", "a/b/c"]);
        let p = go(vec![], vec![ld("x"), ld("x/y")], vec![kd("x"), kd("x/y")]);
        let order: Vec<String> = p.actions.iter().map(|a| match a { Action::RmdirLocal { local, .. } => local.clone(), _ => String::new() }).collect();
        assert_eq!(order, ["x/y", "x"]);
    }

    #[test]
    fn a_file_against_a_folder_touches_neither() {
        let p = go(vec![r("thing", 1, 1)], vec![ld("thing")], vec![]);
        assert!(p.actions.is_empty() && matches!(p.issues[0], Issue::FileVersusFolder { .. }));
    }

    #[test]
    fn a_read_only_server_only_flows_down() {
        let wanted = BTreeSet::new();
        let st = State { files: vec![known("a.txt", 1, 10, 1, 10)].into_iter().collect(), ..Default::default() };
        let local: BTreeMap<_, _> = vec![l("a.txt", 9, 90), l("mine.txt", 1, 1)].into_iter().collect();
        let p = plan(&[r("a.txt", 1, 10)], &local, &st, &Options { case_insensitive: false, windows_names: false, big: 0, wanted: &wanted, can_write: false });
        assert_eq!(kinds(&p), ["Download"], "the edit here is put back; the new file stays here only: {p:?}");
    }
}
