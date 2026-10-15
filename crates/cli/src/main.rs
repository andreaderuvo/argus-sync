//! `argus-sync` without a window: the same engine as the app, for a terminal or a server.
//!
//!     argus-sync connect '<link from Argus>' <folder here>
//!     argus-sync list
//!     argus-sync run              # keeps every folder in sync until stopped
//!     argus-sync once             # one round, then exits
//!     argus-sync fetch <name> <path>   # a big file, asked for
//!     argus-sync pause|resume|forget <name>

use argus_sync_core::engine::{host_name, Engine, Event, Settings};
use argus_sync_core::pair::{self, Pair, Pairs};
use argus_sync_core::plan::Issue;
use argus_sync_core::remote::Remote;
use notify::{RecursiveMode, Watcher};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_millis(700);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = pair::config_dir();
    let result = match args.first().map(String::as_str) {
        Some("connect") if args.len() == 3 => connect(&dir, &args[1], PathBuf::from(&args[2])),
        Some("list") => list(&dir),
        Some("once") => run(&dir, true),
        Some("run") => run(&dir, false),
        Some("fetch") if args.len() == 3 => edit(&dir, &args[1], |p| {
            p.wanted.insert(argus_sync_core::names::key(&args[2]));
        }),
        Some("pause") if args.len() == 2 => edit(&dir, &args[1], |p| p.paused = true),
        Some("resume") if args.len() == 2 => edit(&dir, &args[1], |p| p.paused = false),
        Some("forget") if args.len() == 2 => forget(&dir, &args[1]),
        _ => {
            eprintln!("{}", include_str!("usage.txt"));
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("argus-sync: {e}");
            ExitCode::FAILURE
        }
    }
}

type Res = Result<(), String>;

fn connect(dir: &PathBuf, link: &str, local: PathBuf) -> Res {
    let invite = pair::parse_link(link).ok_or("that is not a link from Argus (argus-sync://pair?url=…&key=…)")?;
    let hello = Remote::new(&invite.url, &invite.key).hello().map_err(|e| e.to_string())?;
    let local = std::path::absolute(&local).map_err(|e| e.to_string())?;
    let mut pairs = Pairs::load(dir);
    if pairs.pairs.iter().any(|p| p.local == local) {
        return Err(format!("{} is already in sync with something", local.display()));
    }
    let made = pairs.add(&invite, local).clone();
    pairs.save(dir).map_err(|e| e.to_string())?;
    println!("connected: {} on {} ({}) ⇄ {}", hello.folder, hello.host, if hello.write { "both ways" } else { "read only" }, made.local.display());
    println!("run `argus-sync run` to keep it in sync");
    Ok(())
}

fn list(dir: &PathBuf) -> Res {
    let pairs = Pairs::load(dir);
    if pairs.pairs.is_empty() {
        println!("nothing connected yet — in Argus, a folder's menu → Sync to your computer…");
    }
    for p in &pairs.pairs {
        println!("{}  {}  ⇄  {}{}", p.name, p.url, p.local.display(), if p.paused { "  (paused)" } else { "" });
    }
    Ok(())
}

fn find<'a>(pairs: &'a mut Pairs, name: &str) -> Result<&'a mut Pair, String> {
    pairs.pairs.iter_mut().find(|p| p.name == name || p.id == name).ok_or_else(|| format!("no folder called {name:?} (see `argus-sync list`)"))
}

fn edit(dir: &PathBuf, name: &str, f: impl FnOnce(&mut Pair)) -> Res {
    let mut pairs = Pairs::load(dir);
    f(find(&mut pairs, name)?);
    pairs.save(dir).map_err(|e| e.to_string())
}

fn forget(dir: &PathBuf, name: &str) -> Res {
    let mut pairs = Pairs::load(dir);
    let p = find(&mut pairs, name)?.clone();
    pairs.pairs.retain(|x| x.id != p.id);
    let _ = std::fs::remove_file(pair::state_file(dir, &p));
    pairs.save(dir).map_err(|e| e.to_string())?;
    println!("forgotten: {} — the files stay where they are, on both sides", p.name);
    Ok(())
}

fn engine_for(dir: &PathBuf, p: &Pair) -> Engine {
    Engine::new(Remote::new(&p.url, &p.key), Settings {
        root: p.local.clone(),
        state_file: pair::state_file(dir, p),
        fallback_trash: pair::fallback_trash(dir, p),
        big: p.big,
        wanted: p.wanted.clone(),
        host: host_name(),
    })
}

fn say(name: &str, ev: &Event) {
    let line = match ev {
        Event::Downloaded { path, bytes } => format!("↓ {path} ({bytes} B)"),
        Event::Uploaded { path, bytes } => format!("↑ {path} ({bytes} B)"),
        Event::DeletedHere { path } => format!("✕ here {path} (to the trash)"),
        Event::DeletedThere { path } => format!("✕ there {path} (to Argus's trash)"),
        Event::FolderHere { path } => format!("+ here {path}/"),
        Event::FolderThere { path } => format!("+ there {path}/"),
        Event::Conflict { path, copy } => format!("⚠ {path}: changed on both sides — yours kept as {copy}"),
        Event::Later { path, why } => format!("… {path} later: {why}"),
    };
    println!("[{name}] {line}");
}

fn tell(name: &str, issues: &[Issue], told: &mut std::collections::BTreeSet<String>) {
    for i in issues {
        let line = match i {
            Issue::Big { remote, size } => format!("{remote} is {} MB — `argus-sync fetch {name} '{remote}'` to bring it", size / 1_000_000),
            Issue::BadName { remote, why } => format!("{remote} cannot exist here: {why}"),
            Issue::CaseClash { kept, skipped } => format!("{skipped} differs from {kept} only by case: this computer keeps {kept}"),
            Issue::FileVersusFolder { path } => format!("{path} is a file on one side and a folder on the other: left alone"),
        };
        if told.insert(line.clone()) {
            println!("[{name}] note: {line}");
        }
    }
}

fn run(dir: &PathBuf, once: bool) -> Res {
    let pairs: Vec<Pair> = Pairs::load(dir).pairs.into_iter().filter(|p| !p.paused).collect();
    if pairs.is_empty() {
        return Err("nothing to sync — connect a folder first (argus-sync connect '<link>' <folder>)".into());
    }
    let mut engines: Vec<(Pair, Engine, Instant, bool)> = pairs.into_iter().map(|p| {
        let e = engine_for(dir, &p);
        (p, e, Instant::now() - POLL, true)
    }).collect();
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
        if let Ok(ev) = ev {
            let _ = tx.send(ev.paths);
        }
    }).map_err(|e| e.to_string())?;
    for (p, ..) in &engines {
        std::fs::create_dir_all(&p.local).map_err(|e| e.to_string())?;
        if let Err(e) = watcher.watch(&p.local, RecursiveMode::Recursive) {
            eprintln!("[{}] not watching for changes ({e}) — checking every {}s instead", p.name, POLL.as_secs());
        }
    }
    let mut told = std::collections::BTreeSet::new();
    loop {
        for (p, e, last, dirty) in engines.iter_mut() {
            if !*dirty && last.elapsed() < POLL {
                continue;
            }
            *last = Instant::now();
            *dirty = false;
            let name = p.name.clone();
            match e.sync_once(&mut |ev| say(&name, ev)) {
                Ok(r) => tell(&p.name, &r.issues, &mut told),
                Err(err) => eprintln!("[{}] {err}", p.name),
            }
        }
        if once {
            return Ok(());
        }
        // Wait for a change here, or the next look at the servers.
        if let Ok(paths) = rx.recv_timeout(Duration::from_millis(500)) {
            let mut touched = paths;
            let until = Instant::now() + SETTLE;
            while let Ok(more) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                touched.extend(more);
            }
            for (p, _, _, dirty) in engines.iter_mut() {
                if touched.iter().any(|t| t.starts_with(&p.local) && !t.file_name().map(|n| argus_sync_core::names::ignored(&n.to_string_lossy())).unwrap_or(false)) {
                    *dirty = true;
                }
            }
        }
    }
}
