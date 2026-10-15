//! argus-sync, the app: the engine of `argus-sync-core` for every connected folder, on a thread of
//! its own, and a window (plain HTML, `../ui`) that shows what it does and asks what to do. It
//! lives in the menu bar / the notification area: closing the window keeps it syncing.

use argus_sync_core::engine::{host_name, Engine, Event, Settings};
use argus_sync_core::names;
use argus_sync_core::pair::{self, Pair, Pairs};
use argus_sync_core::plan::Issue;
use argus_sync_core::remote::{Remote, RemoteError};
use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_opener::OpenerExt;

mod words;
use words::{say, tr, tr_n};

const POLL: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_millis(700);
const RECENT: usize = 60;
const CONFLICT_MARK: &str = " (conflict from ";

// ------------------------------------------------------------------------------- what is shown

#[derive(Debug, Clone, Serialize, Default)]
struct Activity {
    at: String,
    kind: String,
    path: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize, Default)]
struct Waiting {
    path: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
struct Status {
    /// idle | syncing | offline | refused | paused | starting
    state: String,
    message: String,
    last_ok: Option<String>,
    recent: VecDeque<Activity>,
    big: Vec<Waiting>,
    notes: Vec<serde_json::Value>,
    conflicts: Vec<String>,
    write: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct PairView {
    id: String,
    name: String,
    url: String,
    host: String,
    local: String,
    paused: bool,
    big_mb: u64,
    status: Status,
}

#[derive(Debug, Clone, Serialize)]
struct Snapshot {
    pairs: Vec<PairView>,
    paused_all: bool,
    computer: String,
}

#[derive(Debug, Clone, Serialize)]
struct InviteView {
    url: String,
    host: String,
    name: String,
    folder: String,
    write: bool,
    suggested: String,
}

// ------------------------------------------------------------------------------------- shared

enum Wake {
    Now,
}

struct Shared {
    dir: PathBuf,
    status: Mutex<BTreeMap<String, Status>>,
    invite: Mutex<Option<String>>,
    paused_all: Mutex<bool>,
    wake: Mutex<Option<mpsc::Sender<Wake>>>,
}

impl Shared {
    fn nudge(&self) {
        if let Some(tx) = self.wake.lock().unwrap().as_ref() {
            let _ = tx.send(Wake::Now);
        }
    }
}

fn host_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split(['/', ':']).next().unwrap_or(url).to_string()
}

fn snapshot(shared: &Shared) -> Snapshot {
    let pairs = Pairs::load(&shared.dir).pairs;
    let status = shared.status.lock().unwrap();
    Snapshot {
        pairs: pairs.into_iter().map(|p| PairView {
            status: status.get(&p.id).cloned().unwrap_or_else(|| Status { state: if p.paused { "paused".into() } else { "starting".into() }, ..Default::default() }),
            host: host_of(&p.url),
            id: p.id, name: p.name, url: p.url, local: p.local.display().to_string(), paused: p.paused, big_mb: p.big / (1024 * 1024),
        }).collect(),
        paused_all: *shared.paused_all.lock().unwrap(),
        computer: host_name(),
    }
}

fn publish(app: &AppHandle, shared: &Shared) {
    let snap = snapshot(shared);
    // The tray says the one thing worth knowing at a glance.
    let busy = snap.pairs.iter().filter(|p| p.status.state == "syncing").count();
    let conflicts: usize = snap.pairs.iter().map(|p| p.status.conflicts.len()).sum();
    let off = snap.pairs.iter().filter(|p| matches!(p.status.state.as_str(), "offline" | "refused")).count();
    let tip = if snap.pairs.is_empty() {
        tr("argus-sync — nothing connected")
    } else if snap.paused_all {
        tr("argus-sync — paused")
    } else if conflicts > 0 {
        tr_n("argus-sync — {n} conflict(s) to look at", conflicts)
    } else if off > 0 {
        tr_n("argus-sync — {n} folder(s) cannot reach the server", off)
    } else if busy > 0 {
        tr("argus-sync — syncing…")
    } else {
        tr("argus-sync — up to date")
    };
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(&tip));
    }
    let _ = app.emit("snapshot", snap);
}

/// What a failure says, for the window to put in the person's language.
fn remote_say(e: &RemoteError) -> String {
    match e {
        RemoteError::Network(why) => say("Cannot reach the server: {why}", &[("why", why.clone())]),
        RemoteError::Unauthorized => say("The server no longer accepts this key — connect the folder again from Argus", &[]),
        RemoteError::Refused(code, why) => say("The server refused ({code}): {why}", &[("code", code.to_string()), ("why", why.clone())]),
        RemoteError::Conflict(why) => say("Changed on the server meanwhile: {why}", &[("why", why.clone())]),
        RemoteError::Io(why) => say("Something went wrong: {why}", &[("why", why.to_string())]),
    }
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn activity(ev: &Event) -> Activity {
    let (kind, path, detail) = match ev {
        Event::Downloaded { path, bytes } => ("down", path.clone(), bytes.to_string()),
        Event::Uploaded { path, bytes } => ("up", path.clone(), bytes.to_string()),
        Event::DeletedHere { path } => ("deleted-here", path.clone(), String::new()),
        Event::DeletedThere { path } => ("deleted-there", path.clone(), String::new()),
        Event::FolderHere { path } => ("folder-here", path.clone(), String::new()),
        Event::FolderThere { path } => ("folder-there", path.clone(), String::new()),
        Event::Conflict { path, copy } => ("conflict", path.clone(), copy.clone()),
        Event::Later { path, why } => ("later", path.clone(), why.clone()),
    };
    Activity { at: now(), kind: kind.into(), path, detail }
}

fn note(issue: &Issue) -> Option<serde_json::Value> {
    // Structured, so the window says it in the person's language.
    Some(match issue {
        Issue::Big { .. } => return None,
        Issue::BadName { remote, why } => serde_json::json!({"kind": "name", "path": remote, "why": why}),
        Issue::CaseClash { kept, skipped } => serde_json::json!({"kind": "case", "kept": kept, "skipped": skipped}),
        Issue::FileVersusFolder { path } => serde_json::json!({"kind": "type", "path": path}),
    })
}

/// The conflict copies in the folder: what argus-sync made, still waiting for a decision.
fn conflicts_in(engine: &Engine) -> Vec<String> {
    let mut out: Vec<String> = engine.state.files.values()
        .filter(|k| !k.dir && k.local.contains(CONFLICT_MARK))
        .map(|k| k.local.clone())
        .filter(|rel| argus_sync_core::local::join(&engine.settings.root, rel).exists())
        .collect();
    out.sort();
    out
}

// ------------------------------------------------------------------------------------- worker

fn worker(app: AppHandle, shared: Arc<Shared>, rx: mpsc::Receiver<Wake>) {
    let (ftx, frx) = mpsc::channel::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
        if let Ok(ev) = ev {
            let _ = ftx.send(ev.paths);
        }
    }).ok();
    let mut watched: BTreeSet<PathBuf> = BTreeSet::new();
    let mut engines: HashMap<String, (Engine, Instant, bool)> = HashMap::new();
    let mut local_only_conflicts: HashMap<String, BTreeSet<String>> = HashMap::new();

    loop {
        let pairs = Pairs::load(&shared.dir).pairs;
        let paused_all = *shared.paused_all.lock().unwrap();
        engines.retain(|id, _| pairs.iter().any(|p| &p.id == id));
        shared.status.lock().unwrap().retain(|id, _| pairs.iter().any(|p| &p.id == id));
        // Watch what is connected, and only that.
        let want: BTreeSet<PathBuf> = pairs.iter().map(|p| p.local.clone()).collect();
        if let Some(w) = watcher.as_mut() {
            for gone in watched.difference(&want).cloned().collect::<Vec<_>>() {
                let _ = w.unwatch(&gone);
                watched.remove(&gone);
            }
            for new in want.difference(&watched).cloned().collect::<Vec<_>>() {
                let _ = std::fs::create_dir_all(&new);
                if w.watch(&new, RecursiveMode::Recursive).is_ok() {
                    watched.insert(new);
                }
            }
        }

        for p in &pairs {
            let entry = engines.entry(p.id.clone()).or_insert_with(|| (make_engine(&shared.dir, p), Instant::now() - POLL, true));
            entry.0.settings.big = p.big;
            entry.0.settings.wanted = p.wanted.clone();
            if p.paused || paused_all {
                set(&shared, &p.id, |s| { s.state = "paused".into(); s.message.clear(); });
                continue;
            }
            let (engine, last, dirty) = entry;
            if !*dirty && last.elapsed() < POLL {
                continue;
            }
            *last = Instant::now();
            *dirty = false;
            set(&shared, &p.id, |s| if s.state != "syncing" { s.state = "syncing".into() });
            let mut seen: Vec<Activity> = Vec::new();
            let mut copies: Vec<String> = Vec::new();
            let result = engine.sync_once(&mut |ev| {
                if let Event::Conflict { copy, .. } = ev {
                    copies.push(copy.clone());
                }
                seen.push(activity(ev));
            });
            let mine = local_only_conflicts.entry(p.id.clone()).or_default();
            mine.extend(copies);
            mine.retain(|rel| argus_sync_core::local::join(&p.local, rel).exists());
            let mut conflicts = conflicts_in(engine);
            conflicts.extend(mine.iter().cloned());
            conflicts.sort();
            conflicts.dedup();
            let was_idle = seen.is_empty();
            set(&shared, &p.id, |s| {
                for a in seen {
                    s.recent.push_front(a);
                }
                s.recent.truncate(RECENT);
                s.conflicts = conflicts;
                match &result {
                    Ok(r) => {
                        s.state = "idle".into();
                        s.message.clear();
                        s.last_ok = Some(now());
                        s.big = r.issues.iter().filter_map(|i| match i { Issue::Big { remote, size } => Some(Waiting { path: remote.clone(), size: *size }), _ => None }).collect();
                        s.notes = r.issues.iter().filter_map(note).collect();
                    }
                    Err(e) => {
                        s.state = if matches!(e, RemoteError::Unauthorized) { "refused" } else { "offline" }.into();
                        s.message = remote_say(e);
                    }
                }
            });
            if let Ok(h) = engine.remote.hello() {
                set(&shared, &p.id, |s| s.write = Some(h.write));
            }
            publish(&app, &shared);
            // Something moved: look again soon, there is often a second half to it.
            if !was_idle {
                *dirty = true;
            }
        }
        publish(&app, &shared);

        // Wait for a change here, a press in the window, or the next look at the servers.
        let deadline = Instant::now() + Duration::from_millis(500);
        if rx.recv_timeout(Duration::from_millis(10)).is_ok() {
            for (e, _, d) in engines.values_mut() {
                let _ = e;
                *d = true;
            }
            continue;
        }
        if let Ok(paths) = frx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            let mut touched = paths;
            let until = Instant::now() + SETTLE;
            while let Ok(more) = frx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                touched.extend(more);
            }
            for p in &pairs {
                if touched.iter().any(|t| t.starts_with(&p.local) && !t.file_name().map(|n| names::ignored(&n.to_string_lossy())).unwrap_or(false)) {
                    if let Some(e) = engines.get_mut(&p.id) {
                        e.2 = true;
                    }
                }
            }
        }
    }
}

fn set(shared: &Shared, id: &str, f: impl FnOnce(&mut Status)) {
    let mut all = shared.status.lock().unwrap();
    f(all.entry(id.to_string()).or_default());
}

fn make_engine(dir: &Path, p: &Pair) -> Engine {
    Engine::new(Remote::new(&p.url, &p.key), Settings {
        root: p.local.clone(),
        state_file: pair::state_file(dir, p),
        fallback_trash: pair::fallback_trash(dir, p),
        big: p.big,
        wanted: p.wanted.clone(),
        host: host_name(),
    })
}

// ----------------------------------------------------------------------------------- commands

type Res<T> = Result<T, String>;

fn edit(shared: &Shared, id: &str, f: impl FnOnce(&mut Pair) -> Res<()>) -> Res<()> {
    let mut pairs = Pairs::load(&shared.dir);
    let p = pairs.pairs.iter_mut().find(|p| p.id == id).ok_or_else(|| say("That folder is not connected any more", &[]))?;
    f(p)?;
    pairs.save(&shared.dir).map_err(|e| e.to_string())?;
    shared.nudge();
    Ok(())
}

fn pair_of(shared: &Shared, id: &str) -> Res<Pair> {
    Pairs::load(&shared.dir).pairs.into_iter().find(|p| p.id == id).ok_or_else(|| say("That folder is not connected any more", &[]))
}

#[tauri::command]
fn get_snapshot(shared: tauri::State<Arc<Shared>>) -> Snapshot {
    snapshot(&shared)
}

#[tauri::command]
fn take_invite(shared: tauri::State<Arc<Shared>>) -> Option<String> {
    shared.invite.lock().unwrap().take()
}

/// What a link opens, asked of the server before anything is written: so the window can say
/// "proj on lab-server, both ways" and suggest where to put it.
#[tauri::command]
async fn inspect(link: String) -> Res<InviteView> {
    let invite = pair::parse_link(&link).ok_or_else(|| say("That is not a link from Argus — it starts with argus-sync://", &[]))?;
    let url = invite.url.clone();
    let key = invite.key.clone();
    let hello = tauri::async_runtime::spawn_blocking(move || Remote::new(&url, &key).hello())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| remote_say(&e))?;
    let base = dirs::home_dir().unwrap_or_default().join("argus-sync");
    let mut suggested = base.join(&hello.folder);
    let mut n = 2;
    while suggested.exists() && std::fs::read_dir(&suggested).map(|mut d| d.next().is_some()).unwrap_or(false) {
        suggested = base.join(format!("{} {n}", hello.folder));
        n += 1;
    }
    Ok(InviteView {
        url: invite.url, host: hello.host, name: if invite.name.is_empty() { hello.folder.clone() } else { invite.name },
        folder: hello.folder, write: hello.write, suggested: suggested.display().to_string(),
    })
}

#[tauri::command]
fn connect(shared: tauri::State<Arc<Shared>>, link: String, local: String) -> Res<String> {
    let invite = pair::parse_link(&link).ok_or_else(|| say("That is not a link from Argus — it starts with argus-sync://", &[]))?;
    let local = PathBuf::from(local);
    if !local.is_absolute() {
        return Err(say("Choose a folder on this computer", &[]));
    }
    let mut pairs = Pairs::load(&shared.dir);
    if let Some(p) = pairs.pairs.iter().find(|p| p.local == local || local.starts_with(&p.local) || p.local.starts_with(&local)) {
        return Err(say("{path} is already in sync with {name} — choose another folder", &[("path", local.display().to_string()), ("name", p.name.clone())]));
    }
    std::fs::create_dir_all(&local).map_err(|e| say("Cannot make {path}: {why}", &[("path", local.display().to_string()), ("why", e.to_string())]))?;
    let id = pairs.add(&invite, local).id.clone();
    pairs.save(&shared.dir).map_err(|e| e.to_string())?;
    shared.nudge();
    Ok(id)
}

#[tauri::command]
fn set_paused(shared: tauri::State<Arc<Shared>>, id: String, paused: bool) -> Res<()> {
    edit(&shared, &id, |p| { p.paused = paused; Ok(()) })
}

#[tauri::command]
fn pause_all(app: AppHandle, shared: tauri::State<Arc<Shared>>, paused: bool) {
    *shared.paused_all.lock().unwrap() = paused;
    shared.nudge();
    publish(&app, &shared);
}

#[tauri::command]
fn set_big(shared: tauri::State<Arc<Shared>>, id: String, mb: u64) -> Res<()> {
    edit(&shared, &id, |p| { p.big = mb * 1024 * 1024; Ok(()) })
}

#[tauri::command]
fn fetch(shared: tauri::State<Arc<Shared>>, id: String, path: String) -> Res<()> {
    edit(&shared, &id, |p| { p.wanted.insert(names::key(&path)); Ok(()) })
}

#[tauri::command]
fn sync_now(shared: tauri::State<Arc<Shared>>) {
    shared.nudge();
}

/// Stop syncing a folder. The files stay where they are, on both sides.
#[tauri::command]
fn disconnect(shared: tauri::State<Arc<Shared>>, id: String) -> Res<()> {
    let mut pairs = Pairs::load(&shared.dir);
    let gone = pairs.pairs.iter().find(|p| p.id == id).cloned().ok_or_else(|| say("That folder is not connected any more", &[]))?;
    pairs.pairs.retain(|p| p.id != id);
    pairs.save(&shared.dir).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(pair::state_file(&shared.dir, &gone));
    shared.nudge();
    Ok(())
}

/// Move the folder on this computer — by renaming it, so nothing is downloaded again.
#[tauri::command]
fn move_local(shared: tauri::State<Arc<Shared>>, id: String, to: String) -> Res<()> {
    let p = pair_of(&shared, &id)?;
    let to = PathBuf::from(to);
    let dest = if to.exists() && to.is_dir() && std::fs::read_dir(&to).map(|mut d| d.next().is_some()).unwrap_or(false) {
        to.join(p.local.file_name().unwrap_or_default())
    } else {
        to
    };
    if dest.exists() && std::fs::read_dir(&dest).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(say("{path} is not empty", &[("path", dest.display().to_string())]));
    }
    let _ = std::fs::remove_dir(&dest);
    std::fs::rename(&p.local, &dest).map_err(|e| say("Could not move it there ({why}) — a folder on the same disk works", &[("why", e.to_string())]))?;
    edit(&shared, &id, |x| { x.local = dest; Ok(()) })
}

/// A conflict copy, decided: keep mine (it replaces the other), keep the server's (mine goes to
/// the trash), or keep both (the copy loses its "conflict" in the name).
#[tauri::command]
fn resolve(shared: tauri::State<Arc<Shared>>, id: String, copy: String, choice: String) -> Res<()> {
    let p = pair_of(&shared, &id)?;
    let copy_path = argus_sync_core::local::join(&p.local, &copy);
    let start = copy.find(CONFLICT_MARK).ok_or_else(|| say("That is not a conflict copy", &[]))?;
    let end = copy[start..].find(')').map(|i| start + i + 1).ok_or_else(|| say("That is not a conflict copy", &[]))?;
    let original = format!("{}{}", &copy[..start], &copy[end..]);
    let original_path = argus_sync_core::local::join(&p.local, &original);
    match choice.as_str() {
        "mine" => std::fs::rename(&copy_path, &original_path).map_err(|e| e.to_string())?,
        "theirs" => trash::delete(&copy_path).or_else(|_| std::fs::remove_file(&copy_path)).map_err(|e| e.to_string())?,
        "both" => {
            let host: String = host_name();
            let kept = format!("{} (from {host}){}", &copy[..start], &copy[end..]);
            std::fs::rename(&copy_path, argus_sync_core::local::join(&p.local, &kept)).map_err(|e| e.to_string())?
        }
        _ => return Err(say("That is not a conflict copy", &[])),
    }
    shared.nudge();
    Ok(())
}

/// Open a synced folder, or a file in it, with the system (Finder, Explorer, the file manager).
/// Only inside a connected folder: the window cannot ask for anything else.
#[tauri::command]
fn open_in(app: AppHandle, shared: tauri::State<Arc<Shared>>, id: String, rel: Option<String>) -> Res<()> {
    let p = pair_of(&shared, &id)?;
    let rel = rel.unwrap_or_default();
    if rel.split('/').any(|part| part == "..") {
        return Err(say("That is not inside the synced folder", &[]));
    }
    let path = if rel.is_empty() { p.local.clone() } else { argus_sync_core::local::join(&p.local, &rel) };
    if !path.exists() {
        return Err(say("{path} is not there", &[("path", path.display().to_string())]));
    }
    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| say("Something went wrong: {why}", &[("why", e.to_string())]))
}

// ----------------------------------------------------------------------------------------- app

fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn take_links(app: &AppHandle, shared: &Shared, urls: Vec<String>) {
    for url in urls.into_iter().filter(|u| u.starts_with("argus-sync:")) {
        *shared.invite.lock().unwrap() = Some(url.clone());
        let _ = app.emit("invite", url);
    }
    show(app);
}

pub fn run() {
    let dir = pair::config_dir();
    let shared = Arc::new(Shared {
        dir,
        status: Mutex::new(BTreeMap::new()),
        invite: Mutex::new(None),
        paused_all: Mutex::new(false),
        wake: Mutex::new(None),
    });
    let for_instance = shared.clone();
    tauri::Builder::default()
        // A second launch (a link clicked while the app runs) hands its link to the first.
        .plugin(tauri_plugin_single_instance::init(move |app, argv, _cwd| {
            take_links(app, &for_instance, argv.into_iter().skip(1).collect());
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![
            get_snapshot, take_invite, inspect, connect, set_paused, pause_all, set_big, fetch, sync_now,
            disconnect, move_local, resolve, open_in
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            #[cfg(any(windows, target_os = "linux"))]
            {
                // Installers register the scheme; this makes a copy run from anywhere answer too.
                let _ = app.deep_link().register_all();
            }
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                take_links(&handle, &shared, urls.into_iter().map(|u| u.to_string()).collect());
            }
            let on_link = shared.clone();
            let h2 = handle.clone();
            app.deep_link().on_open_url(move |event| {
                take_links(&h2, &on_link, event.urls().into_iter().map(|u| u.to_string()).collect());
            });

            let open = MenuItem::with_id(app, "open", tr("Open argus-sync"), true, None::<&str>)?;
            let now = MenuItem::with_id(app, "now", tr("Sync now"), true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", tr("Quit"), true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &now, &quit])?;
            let tray_shared = shared.clone();
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("argus-sync")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "open" => show(app),
                    "now" => tray_shared.nudge(),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            if std::env::args().any(|a| a == "--hidden") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }

            let (tx, rx) = mpsc::channel();
            *shared.wake.lock().unwrap() = Some(tx);
            let worker_shared = shared.clone();
            std::thread::spawn(move || worker(handle, worker_shared, rx));
            Ok(())
        })
        // Closing the window keeps it syncing, in the menu bar / notification area.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("argus-sync could not start");
}
