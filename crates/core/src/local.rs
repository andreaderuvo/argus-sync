//! The computer's end: the folder as it is on disk, and the file operations argus-sync does there,
//! each made safe on its own (a download lands beside its target and is renamed over it; a delete
//! goes to the system's trash).

use crate::names;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq)]
pub struct LocalEntry {
    /// The path as it is on disk, with `/` — its Unicode form untouched.
    pub real: String,
    pub dir: bool,
    pub size: u64,
    pub mtime: i64,
}

pub fn mtime_ns(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// `rel` (with `/`) under `root`, with this computer's separator.
pub fn join(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    p
}

/// Everything under `root`, keyed by [`names::key`]. Symbolic links are neither followed nor
/// listed (one pointing outside would sync what is not this folder; one in a loop never ends).
pub fn scan(root: &Path) -> io::Result<BTreeMap<String, LocalEntry>> {
    let mut out = BTreeMap::new();
    let walker = walkdir::WalkDir::new(root).follow_links(false).min_depth(1).into_iter();
    for item in walker.filter_entry(|e| !names::ignored(&e.file_name().to_string_lossy())) {
        let e = match item {
            Ok(e) => e,
            Err(err) if err.io_error().map(|x| x.kind() == io::ErrorKind::PermissionDenied).unwrap_or(false) => continue,
            Err(err) => return Err(err.into()),
        };
        if e.path_is_symlink() {
            continue;
        }
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,                       // gone between the listing and the look
        };
        let rel = e.path().strip_prefix(root).expect("under root");
        let real = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
        let dir = meta.is_dir();
        out.insert(names::key(&real), LocalEntry { real, dir, size: if dir { 0 } else { meta.len() }, mtime: if dir { 0 } else { mtime_ns(&meta) } });
    }
    Ok(out)
}

/// Whether this folder's file system tells `A` from `a`. Asked of the disk, not of the OS: a Mac
/// can format a volume case-sensitive, and Linux can mount a case-insensitive one.
pub fn case_insensitive(root: &Path) -> bool {
    let probe = root.join(format!(".argus-sync-CaseProbe-{}", rand::random::<u32>()));
    if fs::write(&probe, b"").is_err() {
        return cfg!(any(target_os = "macos", windows));
    }
    let lower = root.join(probe.file_name().unwrap().to_string_lossy().to_lowercase());
    let answer = lower.exists();
    let _ = fs::remove_file(&probe);
    answer
}

/// A temporary file beside `target`, in the same folder so the final rename stays on one disk.
pub fn part_beside(target: &Path) -> PathBuf {
    let dir = target.parent().unwrap_or(Path::new("."));
    dir.join(format!(".argus-sync-{:08x}.part", rand::random::<u32>()))
}

/// Put `part` in place of `target`, with the server's time. On Windows a file open in another
/// program cannot be replaced: that comes back as an error and is retried on the next round.
pub fn settle(part: &Path, target: &Path, mtime: i64) -> io::Result<fs::Metadata> {
    let t = filetime::FileTime::from_unix_time(mtime.div_euclid(1_000_000_000), mtime.rem_euclid(1_000_000_000) as u32);
    filetime::set_file_mtime(part, t)?;
    fs::rename(part, target)?;
    fs::metadata(target)
}

/// To the system's trash (Finder's, the Recycle Bin, freedesktop's), so a delete that came from
/// the server can be undone here. Where there is no trash (a server without a desktop), into
/// `fallback`, a folder argus-sync keeps for the purpose.
pub fn to_trash(path: &Path, fallback: &Path) -> io::Result<()> {
    if trash::delete(path).is_ok() {
        return Ok(());
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let dest = fallback.join(stamp.to_string());
    fs::create_dir_all(&dest)?;
    fs::rename(path, dest.join(path.file_name().unwrap_or_default()))
}

/// Byte for byte.
pub fn same_content(a: &Path, b: &Path) -> io::Result<bool> {
    use std::io::Read;
    let (ma, mb) = (fs::metadata(a)?, fs::metadata(b)?);
    if ma.len() != mb.len() {
        return Ok(false);
    }
    let (mut fa, mut fb) = (fs::File::open(a)?, fs::File::open(b)?);
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let n = fa.read(&mut ba)?;
        if n == 0 {
            return Ok(true);
        }
        fb.read_exact(&mut bb[..n])?;
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
    }
}
