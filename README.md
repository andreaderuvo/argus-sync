# argus-sync

A folder of an [Argus](https://github.com/andreaderuvo/argus) server, kept in sync with a folder on
your computer — both ways, like Dropbox, on **macOS, Windows and Linux**. Files are real files on your
disk: they open with a double-click, work offline, and what you change goes back to the server.

- **No administrator, no SSH key, no FUSE**: an app you install for yourself.
- **Connect with a click**: in Argus, a folder's menu → *Sync to your computer…* gives a link; it opens
  argus-sync already pointed at that folder. You choose where it goes on your computer.
- **A key that opens one folder**: no terminal, no sessions, no other file. Taken back from Argus.
- **Nothing is lost**: a file changed on both sides is kept twice (`report (conflict from laptop …).pdf`);
  a delete goes to the trash (yours, or Argus's); a file over the size limit (500 MB by default) waits
  to be asked for.

## Cross-OS, by design

| | handled |
|---|---|
| `Report.pdf` and `report.pdf` on the server, a case-insensitive disk here | one is synced, the other is reported |
| `è` written by a Mac (NFD) and by Linux (NFC) | the same file |
| names Windows refuses (`: * ? " < > \|`, `CON`, `NUL`, trailing dot…) | reported, never renamed silently |
| file times kept at different precisions (APFS, NTFS, exFAT) | each side compared with itself only |
| a file open in another program (Windows) | retried on the next round |
| `.DS_Store`, `Thumbs.db`, `desktop.ini`, Office/LibreOffice lock files | ignored everywhere |

## Install

Download the installer for your system from the
[latest release](https://github.com/andreaderuvo/argus-sync/releases/latest):

| system | file | first launch |
|---|---|---|
| macOS (Apple silicon and Intel) | `.dmg` | the app is not signed by Apple yet: right-click it → **Open** → **Open**, once |
| Windows 10/11 | `.exe` (or `.msi`) | SmartScreen may warn: **More info** → **Run anyway** |
| Linux | `.AppImage`, `.deb` or `.rpm` | `chmod +x` the AppImage, or install the package |

No administrator rights are needed on macOS (drag it into your own Applications) or with the
Windows `.exe`. Then, in Argus, open a folder's menu → **Sync to your computer…** and click the link.

It needs an Argus with the sync routes (`/api/sync/*`), from the release after 2026-10-15 onwards.

## Security

- A sync key opens **one folder** and nothing else: it cannot reach a terminal, a session, the
  settings or another folder; read-only unless you allow writing when you make it. Argus keeps only a
  hash of it, and you can take it back from the same menu at any time.
- The key travels with every request. Over plain `http` on a network you do not trust, anyone on it
  can read it: use Argus over `https` (`tls_cert`), a VPN such as Tailscale, or an SSH tunnel.
- Writes obey the server's upload limit (`max_upload_bytes`); deletes made through a key go to a trash
  beside Argus's config, never straight to nothing.

## Layout

```
crates/core   the engine: names.rs (cross-OS rules), plan.rs (what to do — pure, tested case by case),
              engine.rs (doing it, safely), remote.rs (Argus's /api/sync), local.rs, state.rs, pair.rs
crates/cli    argus-sync without a window: connect, list, run, once, fetch, pause, resume, forget
app/          the app (Tauri 2): src-tauri/ the Rust side (tray, deep link argus-sync://, the worker),
              ui/ the window — plain HTML/JS, no bundler; ui/mock.js lets it run in a browser
tests/ui      the window in headless Chromium, against the mock
```

## Build and test

```bash
cargo test -p argus-sync-core                  # unit tests + against a real Argus (ARGUS_SRC=<argus checkout>)
cargo build --release -p argus-sync-cli        # target/release/argus-sync
python -m pytest -q tests/ui                   # the window (needs Chromium)
cd app/src-tauri && cargo build                # the app (Linux: WebKitGTK 4.1 dev files)
```

The installers (`.dmg`, `.msi`/`.exe`, AppImage/`.deb`/`.rpm`) are built by CI on each system.

## License

MIT — see [LICENSE](LICENSE).
