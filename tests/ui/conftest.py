"""The window, in a headless Chromium, against the pretend app (ui/mock.js). The same driver as
Argus's browser tests (cdp.py, copied): any console error or failed request of ours fails a test."""

from __future__ import annotations

import functools
import glob
import http.server
import os
import shutil
import signal
import subprocess
import threading
import time
from pathlib import Path

import pytest

from .cdp import DESKTOP, Page, Session, http_json

UI = Path(__file__).resolve().parents[2] / "app" / "ui"


def find_chromium() -> str | None:
    said = os.environ.get("ARGUS_CHROMIUM")
    if said:
        return said
    for pattern in (str(Path.home() / ".cache/ms-playwright/chromium-*/chrome-linux*/chrome"),
                    str(Path.home() / "Library/Caches/ms-playwright/chromium-*/chrome-mac/Chromium.app/Contents/MacOS/Chromium")):
        found = sorted(glob.glob(pattern))
        if found:
            return found[-1]
    for name in ("chromium", "chromium-browser", "google-chrome", "chrome"):
        if shutil.which(name):
            return shutil.which(name)
    return None


@pytest.fixture(scope="session")
def site():
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(UI))
    handler.log_message = lambda *a: None
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_address[1]}"
    srv.shutdown()


@pytest.fixture(scope="session")
def chromium(tmp_path_factory):
    path = find_chromium()
    if not path:
        pytest.skip("no Chromium (set ARGUS_CHROMIUM)")
    profile = tmp_path_factory.mktemp("chromium")
    proc = subprocess.Popen([path, "--headless=new", "--no-sandbox", "--disable-gpu", "--no-first-run", "--remote-debugging-port=0",
                             f"--user-data-dir={profile}", "about:blank"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    port_file = profile / "DevToolsActivePort"
    end = time.monotonic() + 20
    while not port_file.exists() and time.monotonic() < end:
        time.sleep(0.05)
    port = int(port_file.read_text().split()[0])
    browser = Session(http_json(f"http://127.0.0.1:{port}/json/version")["webSocketDebuggerUrl"])
    yield {"port": port, "browser": browser}
    browser.close()
    os.killpg(proc.pid, signal.SIGTERM)


@pytest.fixture
def make_page(chromium, site):
    opened = []

    def make(query: str = "", viewport=DESKTOP, lang: str = "en-GB", dark: bool = True) -> Page:
        b = chromium["browser"]
        ctx = b.send("Target.createBrowserContext", {"disposeOnDetach": True})["browserContextId"]
        target = b.send("Target.createTarget", {"url": "about:blank", "browserContextId": ctx})["targetId"]
        page = Page(f"ws://127.0.0.1:{chromium['port']}/devtools/page/{target}", site)
        page.viewport(viewport)
        page.send("Page.addScriptToEvaluateOnNewDocument", {
            "source": f"Object.defineProperty(Navigator.prototype, 'language', {{ get: () => {lang!r} }});"})
        page.send("Emulation.setEmulatedMedia", {"features": [{"name": "prefers-color-scheme", "value": "dark" if dark else "light"}]})
        page.goto(f"{site}/index.html{query}")
        page.wait("!!document.querySelector('#main').children.length", what="the window drawn")
        opened.append((page, target, ctx))
        return page

    yield make
    failures = []
    for page, target, ctx in opened:
        page.settle(quiet=0.3, timeout=3)
        if page.problems:
            failures.append("\n    ".join(page.problems))
        page.close()
        for method, params in (("Target.closeTarget", {"targetId": target}), ("Target.disposeBrowserContext", {"browserContextId": ctx})):
            try:
                chromium["browser"].send(method, params, timeout=5)
            except Exception:
                pass
    if failures:
        pytest.fail("the window reported problems:\n    " + "\n  ---\n    ".join(failures), pytrace=False)
