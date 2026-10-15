"""Nothing in the window is left in English by accident: every key the window or the app's
messages use has an Italian, Spanish and French version, and none of them is just the key again."""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
UI = ROOT / "app" / "ui"
# The same in every language, on purpose.
SAME = {"argus-sync://pair?…", "Pause", "Notes", "Conflict"}
SAME_IN = {"fr": {"Pause", "Notes"}, "es": set(), "it": set()}


def catalogues() -> dict:
    script = ("Object.defineProperty(globalThis, 'navigator', { value: { language: 'en' }, configurable: true }); globalThis.document = { documentElement: {} };"
              f"const m = await import({json.dumps((UI / 'words.js').as_uri())});"
              "console.log(JSON.stringify(m.CATALOGUES));")
    out = subprocess.run(["node", "--input-type=module", "-e", script], capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


def used_keys() -> set[str]:
    js = (UI / "app.js").read_text()
    keys = set(re.findall(r"\bt\('((?:[^'\\]|\\.)*)'", js))
    # Words looked up through a table: the states, the kinds of activity, the reasons for "later".
    for table in ("STATES", "KINDS"):
        block = js[js.index(f"const {table} = {{"):]
        block = block[:block.index("};")]
        keys |= set(re.findall(r"'([A-Z][^']*)'\]", block))
    block = js[js.index("const LATER = ["):]
    block = block[:block.index("];")]
    keys |= {m[1] for m in re.findall(r"\['([^']*)', '([^']*)'\]", block)}
    keys |= {"a character Windows does not allow", "a name ending with a dot or a space", "a name Windows reserves"}
    rs = (ROOT / "app" / "src-tauri" / "src" / "lib.rs").read_text()
    keys |= set(re.findall(r'say\("((?:[^"\\]|\\.)*)"', rs))
    return {k.replace("\\'", "'") for k in keys}


def test_every_word_is_translated():
    cats = catalogues()
    keys = used_keys()
    assert len(keys) > 60, keys
    for code, table in cats.items():
        missing = sorted(k for k in keys if k not in table)
        assert not missing, f"{code}: not translated: {missing}"
        lazy = sorted(k for k in keys if table.get(k) == k and k not in SAME and k not in SAME_IN[code])
        assert not lazy, f"{code}: still the English: {lazy}"


def test_holes_survive_translation():
    for code, table in catalogues().items():
        for key, said in table.items():
            assert sorted(re.findall(r"\{\w+\}", key)) == sorted(re.findall(r"\{\w+\}", said)), f"{code}: {key!r} → {said!r}"
