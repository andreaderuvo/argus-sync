"""A small Chrome DevTools Protocol client, and a page that remembers everything that went wrong.

Built on `websockets`, which uvicorn[standard] already brings, so the browser tests need no
Python package the server does not. The point of `Page` is `problems`: every uncaught exception,
every console error, every failed or 4xx/5xx request of the app's own, collected from the moment
the page opens. The `page` fixture fails a test that leaves any behind — so a test that only
means to check a button also catches the ReferenceError three functions away.
"""

from __future__ import annotations

import itertools
import json
import queue
import threading
import time
from urllib.request import Request, urlopen

from websockets.exceptions import ConnectionClosed
from websockets.sync.client import connect


class CDPError(RuntimeError):
    pass


class Timeout(AssertionError):
    pass


class Session:
    """One websocket to one target: commands with ids, events kept in order."""

    def __init__(self, ws_url: str):
        self.ws = connect(ws_url, max_size=None, open_timeout=15, ping_interval=None)
        self._ids = itertools.count(1)
        self._pending: dict[int, queue.Queue] = {}
        self._lock = threading.Lock()
        self.events: list[dict] = []
        self._listeners: list = []
        self._reader = threading.Thread(target=self._read, daemon=True)
        self._reader.start()

    def _read(self) -> None:
        try:
            for raw in self.ws:
                msg = json.loads(raw)
                if "id" in msg:
                    with self._lock:
                        box = self._pending.pop(msg["id"], None)
                    if box:
                        box.put(msg)
                else:
                    self.events.append(msg)
                    for fn in list(self._listeners):
                        fn(msg)
        except ConnectionClosed:
            pass

    def send(self, method: str, params: dict | None = None, timeout: float = 20.0, session: str | None = None) -> dict:
        n = next(self._ids)
        box: queue.Queue = queue.Queue()
        with self._lock:
            self._pending[n] = box
        # `session`: a frame of another process (a sandboxed viewer pack), attached flat.
        self.ws.send(json.dumps({"id": n, "method": method, "params": params or {},
                                 **({"sessionId": session} if session else {})}))
        try:
            msg = box.get(timeout=timeout)
        except queue.Empty:
            raise Timeout(f"{method}: no answer in {timeout}s") from None
        if "error" in msg:
            raise CDPError(f"{method}: {msg['error']}")
        return msg.get("result", {})

    def close(self) -> None:
        try:
            self.ws.close()
        except Exception:
            pass


def http_json(url: str, method: str = "GET"):
    with urlopen(Request(url, method=method), timeout=10) as r:
        return json.loads(r.read().decode())


PHONE = {"width": 390, "height": 780, "deviceScaleFactor": 2, "mobile": True}
DESKTOP = {"width": 1400, "height": 900, "deviceScaleFactor": 1, "mobile": False}


class Page(Session):
    """A tab of the test browser, pointed at the test Argus.

    `origin` is the app's own origin: only its requests count as problems, so a 404 from
    somewhere else does not fail the suite — and a 404 for one of our own modules does.
    """

    def __init__(self, ws_url: str, origin: str):
        super().__init__(ws_url)
        self.origin = origin
        # Predicates over problem strings a test expects. One is always expected: the wall asks
        # for a desk's PLAN.argus.md / BRIDGE.argus.md to find out whether two agents are paired,
        # and "not there" is the ordinary answer (readPair/readBridge catch it). Only a 404, and
        # only for those two names — any other missing file is still a problem.
        # None any more: the desk asks for PLAN/BRIDGE.argus.md with missing_ok and gets 204, so a
        # 404 for them would now be a real problem.
        self.allowed: list = []
        self._inflight: dict[str, str] = {}
        self._last_net = time.monotonic()
        self._listeners.append(self._track)
        for domain in ("Page", "Runtime", "Log", "Network"):
            self.send(f"{domain}.enable")
        # Bells, notifications and the clipboard would otherwise wait on a permission prompt
        # nobody is there to answer.
        try:
            self.send("Browser.grantPermissions", {
                "origin": origin,
                "permissions": ["notifications", "clipboardReadWrite", "clipboardSanitizedWrite"],
            })
        except CDPError:
            pass

    # ---------------------------------------------------------------- what went wrong

    def _track(self, msg: dict) -> None:
        m, p = msg.get("method"), msg.get("params", {})
        if m == "Network.requestWillBeSent":
            self._inflight[p["requestId"]] = (p["request"]["url"], time.monotonic())
            self._last_net = time.monotonic()
        elif m in ("Network.loadingFinished", "Network.loadingFailed"):
            self._inflight.pop(p["requestId"], None)
            self._last_net = time.monotonic()

    @property
    def problems(self) -> list[str]:
        out: list[str] = []
        urls: dict[str, str] = {}
        for msg in self.events:
            m, p = msg.get("method"), msg.get("params", {})
            if m == "Network.requestWillBeSent":
                urls[p["requestId"]] = p["request"]["url"]
            elif m == "Runtime.exceptionThrown":
                d = p["exceptionDetails"]
                text = (d.get("exception") or {}).get("description") or d.get("text", "")
                where = f"{d.get('url', '')}:{d.get('lineNumber', 0) + 1}"
                out.append(f"exception: {text.splitlines()[0] if text else '?'} @ {where}")
            elif m == "Runtime.consoleAPICalled" and p.get("type") in ("error", "assert"):
                args = " ".join(str(a.get("value", a.get("description", ""))) for a in p.get("args", []))
                out.append(f"console.{p['type']}: {args}")
            elif m == "Log.entryAdded" and p["entry"].get("level") == "error":
                e = p["entry"]
                # network errors are reported below with their status; do not count them twice
                if e.get("source") != "network":
                    out.append(f"log[{e.get('source')}]: {e.get('text')} {e.get('url', '')}".strip())
            elif m == "Network.responseReceived":
                r = p["response"]
                if r["url"].startswith(self.origin) and r["status"] >= 400:
                    out.append(f"http {r['status']}: {r['url'][len(self.origin):]}")
                # A module answered with HTML is a missing file: serve_static falls back to the
                # index page, and the browser then refuses it as "not a module".
                if (r["url"].startswith(self.origin) and r["url"].split("?")[0].endswith(".js")
                        and "html" in r.get("mimeType", "")):
                    out.append(f"script served as html (missing file?): {r['url'][len(self.origin):]}")
            elif m == "Network.loadingFailed":
                url = urls.get(p["requestId"], "")
                if url.startswith(self.origin) and not p.get("canceled") \
                        and "ERR_ABORTED" not in p.get("errorText", ""):
                    out.append(f"failed: {p.get('errorText')} {url[len(self.origin):]}")
        return [x for x in out if not any(ok(x) for ok in self.allowed)]

    def allow(self, fragment: str) -> None:
        """Expect a problem containing this text, for a test that provokes one on purpose."""
        self.allowed.append(lambda s, f=fragment: f in s)

    # ---------------------------------------------------------------- driving

    def viewport(self, spec: dict) -> None:
        self.send("Emulation.setDeviceMetricsOverride", spec)
        # Headless Chromium has no mouse, so it reports neither hover nor a fine pointer —
        # which is neither a desk nor a phone. The browser is started with a mouse declared
        # (--blink-settings, conftest.py); touch emulation turns this page into a phone.
        # Only ever switched *on*: setTouchEmulationEnabled(false) does not restore that
        # mouse, it resets to "no pointer at all" (measured). Each test has a fresh tab.
        self.touch = bool(spec.get("mobile"))
        if self.touch:
            self.send("Emulation.setTouchEmulationEnabled", {"enabled": True, "maxTouchPoints": 5})

    def reload(self, timeout: float = 15.0) -> None:
        """Reload and wait for the *new* document. `location.reload()` returns at once, and a wait
        after it can be answered by the old page still on screen — then that page goes, and the
        next look finds nothing (the rail test, flaky for exactly this)."""
        self.eval("window.__argusOld = true")
        self.send("Page.reload", {"ignoreCache": False})
        self.wait("!window.__argusOld && document.readyState === 'complete'", timeout=timeout, what="the reloaded page")

    def goto(self, url: str) -> None:
        self.send("Page.navigate", {"url": url})
        self.wait("document.readyState === 'complete'")

    def eval(self, expression: str, await_promise: bool = True):
        r = self.send("Runtime.evaluate", {
            "expression": expression, "awaitPromise": await_promise,
            "returnByValue": True, "userGesture": True,
        })
        if "exceptionDetails" in r:
            d = r["exceptionDetails"]
            raise CDPError(f"eval failed: {(d.get('exception') or {}).get('description') or d.get('text')}\n  in: {expression[:200]}")
        return r.get("result", {}).get("value")

    def frame_eval(self, url_part: str, expression: str, timeout: float = 10.0):
        """Evaluate in the frame whose address contains `url_part` — a frame sandboxed at an
        opaque origin, which the page's own eval cannot reach into. Chrome puts such frames in
        a process of their own, so the frame is attached as a target of this session."""
        if not getattr(self, "_auto", False):
            self.send("Target.setAutoAttach", {"autoAttach": True, "waitForDebuggerOnStart": False, "flatten": True})
            self._auto = True
        end = time.monotonic() + timeout
        last = None
        while time.monotonic() < end:
            # A frame is attached before it navigates (its url about:blank) and says where it
            # went later, in targetInfoChanged; one redrawn is detached. Newest first.
            sessions, urls, gone = {}, {}, set()
            for ev in self.events:
                m, p = ev.get("method"), ev.get("params", {})
                if m == "Target.attachedToTarget":
                    sessions[p["targetInfo"]["targetId"]] = p["sessionId"]
                    urls[p["targetInfo"]["targetId"]] = p["targetInfo"].get("url", "")
                elif m == "Target.targetInfoChanged":
                    urls[p["targetInfo"]["targetId"]] = p["targetInfo"].get("url", "")
                elif m == "Target.detachedFromTarget":
                    gone.add(p.get("sessionId"))
            for target, session in reversed(list(sessions.items())):
                if session in gone or url_part not in urls.get(target, ""):
                    continue
                try:
                    r = self.send("Runtime.evaluate", {"expression": expression, "returnByValue": True,
                                                      "awaitPromise": True}, session=session)
                except CDPError as e:
                    last = e                      # a frame redrawn under us: try the next one
                    continue
                if "exceptionDetails" not in r:
                    return r.get("result", {}).get("value")
                last = r["exceptionDetails"]
            time.sleep(0.1)
        raise Timeout(f"no frame {url_part!r} to evaluate in ({last})")

    def wait(self, expression: str, timeout: float = 10.0, what: str | None = None):
        """Until `expression` is truthy *in JavaScript*. Wrapped so that an element counts as
        found: returned by value, a DOM node arrives as `{}`, which Python reads as false."""
        end = time.monotonic() + timeout
        last = None
        probe = f"(() => {{ const v = ({expression}); return (v instanceof Node) ? true : v; }})()"
        while time.monotonic() < end:
            try:
                last = self.eval(probe)
            except CDPError:
                last = None
            if last:
                return last
            time.sleep(0.05)
        raise Timeout(f"waited {timeout}s for: {what or expression}\n  problems so far: {self.problems}")

    def settle(self, quiet: float = 0.35, timeout: float = 10.0) -> None:
        """Until no request of ours has started or finished for `quiet` seconds.

        Pollers (the session count, a folder watcher) fire every few seconds, so quiet means
        a gap between them — which is what a person sees as "it has loaded".
        """
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            now = time.monotonic()
            live = [u for u, since in list(self._inflight.values())
                    if u.startswith(self.origin) and not self._long_lived(u, now - since)]
            if not live and now - self._last_net >= quiet:
                return
            time.sleep(0.05)

    @staticmethod
    def _long_lived(url: str, age: float) -> bool:
        """Connections that stay open by design, and requests a navigation orphaned.

        The bell stream is one fetch held open for the page's whole life; a terminal is a
        websocket. And a request whose document was navigated away never reports finishing.
        Anything open for 3s is treated as one of these rather than as loading.
        """
        return "/ws" in url or "/api/bells/stream" in url or age > 3.0

    def route(self, hash_: str, ready: str | None = None, timeout: float = 10.0) -> None:
        self.eval(f"location.hash = {json.dumps(hash_)}")
        # A screen may rewrite its own address (#/files becomes #/files?path=…), so only the
        # route part has to match.
        path = hash_.split("?")[0]
        self.wait(f"location.hash.split('?')[0] === {json.dumps(path)}", timeout=3)
        if ready:
            self.wait(ready, timeout=timeout)
        self.settle()

    def _center(self, finder: str) -> tuple[float, float]:
        box = self.eval(f"""(() => {{
            const e = {finder};
            if (!e) return null;
            e.scrollIntoView({{block: 'center', inline: 'center'}});
            const r = e.getBoundingClientRect();
            if (!r.width || !r.height) return 'invisible';
            // Layout pixels to screen pixels. They differ when the page is wider than the
            // screen and a phone shrinks it to fit: a tap is delivered in screen pixels, so
            // without this it lands short, further off the lower down the target is.
            const k = (window.visualViewport ? visualViewport.width : innerWidth) / innerWidth;
            return [(r.left + r.width / 2) * k, (r.top + r.height / 2) * k];
        }})()""")
        if box is None:
            raise AssertionError(f"nothing matches {finder}")
        if box == "invisible":
            raise AssertionError(f"{finder} is not visible")
        return box[0], box[1]

    def click_at(self, x: float, y: float, count: int = 1, modifiers: int = 0) -> None:
        """A click where there is a mouse, a tap where there is a finger — as each device sends it.
        `modifiers` as CDP counts them: 1 Alt, 2 Ctrl, 4 Meta, 8 Shift."""
        if getattr(self, "touch", False):
            for _ in range(count):
                self.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x, "y": y}]})
                self.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
            return
        for kind in ("mouseMoved", "mousePressed", "mouseReleased"):
            self.send("Input.dispatchMouseEvent", {
                "type": kind, "x": x, "y": y, "button": "left" if kind != "mouseMoved" else "none",
                "clickCount": count, "modifiers": modifiers,
            })

    def click(self, selector: str) -> None:
        """A real click — pointer events and all — on the first visible match of a CSS selector."""
        self.wait(f"(() => {{ const e = document.querySelector({json.dumps(selector)}); "
                  f"return e && e.getClientRects().length > 0; }})()", what=f"visible {selector}")
        self.click_at(*self._center(f"document.querySelector({json.dumps(selector)})"))

    def click_text(self, text: str, within: str = "button, a, [role=button], .row") -> None:
        """A real click on the visible element whose own text contains `text`."""
        finder = (f"[...document.querySelectorAll({json.dumps(within)})]"
                  f".filter(e => e.getClientRects().length > 0)"
                  f".find(e => e.textContent.replace(/\\s+/g, ' ').includes({json.dumps(text)}))")
        self.wait(f"!!({finder})", what=f"visible element with text {text!r}")
        self.click_at(*self._center(finder))

    def type(self, text: str) -> None:
        self.send("Input.insertText", {"text": text})

    def key(self, key: str, code: str | None = None, modifiers: int = 0, text: str | None = None) -> None:
        vk = {"Enter": 13, "Escape": 27, "Tab": 9, "Backspace": 8, "Delete": 46, "ArrowDown": 40, "ArrowUp": 38, "ArrowLeft": 37, "ArrowRight": 39}
        base = {"key": key, "code": code or key, "modifiers": modifiers,
                "windowsVirtualKeyCode": vk.get(key, ord(key.upper()) if len(key) == 1 else 0)}
        down = {**base, "type": "keyDown"}
        if text is not None:
            down["text"] = text
        elif key == "Enter":
            down["text"] = "\r"
        self.send("Input.dispatchKeyEvent", down)
        self.send("Input.dispatchKeyEvent", {**base, "type": "keyUp"})

    def text(self, selector: str = "body") -> str:
        return self.eval(f"document.querySelector({json.dumps(selector)})?.innerText || ''")

    def count(self, selector: str) -> int:
        return self.eval(f"document.querySelectorAll({json.dumps(selector)}).length")

    def screenshot(self, path) -> None:
        import base64
        data = self.send("Page.captureScreenshot", {"format": "png"})["data"]
        with open(path, "wb") as f:
            f.write(base64.b64decode(data))
