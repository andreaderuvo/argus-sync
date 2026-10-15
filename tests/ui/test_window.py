"""What the window shows and what its buttons ask of the app."""

from __future__ import annotations

from .cdp import Page

PHONE_WIDE = {"width": 420, "height": 760, "deviceScaleFactor": 1, "mobile": False}
CALLS = "window.__mock.calls.map(c => c[0])"


def test_every_folder_with_its_state_conflicts_and_waiting_files(make_page):
    page: Page = make_page()
    assert page.count(".card") == 2
    first = page.eval("document.querySelector('.card').textContent")
    for words in ("report-salmonella", "on lab-server", "Up to date", "Conflicts", "Keep mine", "Waiting to be asked for",
                  "S1_R1.fastq.gz", "3.9 GB", "Notes", "Recent activity (5)"):
        assert words in first, words
    second = page.eval("document.querySelectorAll('.card')[1].textContent")
    assert "Syncing…" in second and "read only" in second
    assert page.eval("document.querySelector('#computer').textContent") == "on lap684"


def test_a_conflict_decided_and_a_big_file_fetched(make_page):
    page = make_page()
    page.click_text("Keep mine")
    page.wait("!document.querySelector('.sect.alert')", what="the conflict gone from the list")
    page.click_text("Fetch")
    page.wait("[...document.querySelectorAll('.card')][0].textContent.includes('Waiting to be asked for1')", what="one left")
    calls = page.eval("JSON.stringify(window.__mock.calls)")
    assert '"resolve",{"id":"a1","copy":"tables/summary (conflict from lap684 2026-10-14 1128).xlsx","choice":"mine"}' in calls
    assert '"fetch",{"id":"a1","path":"reads/S1_R1.fastq.gz"}' in calls


def test_connecting_from_a_pasted_link(make_page):
    page = make_page("?empty")
    assert "No folder connected yet" in page.text("#main")
    page.click("#main input")
    page.type("argus-sync://pair?url=http%3A%2F%2Flab-server%3A8090&key=as_x")
    page.click_text("Continue")
    page.wait("!!document.querySelector('.found')", what="what the link opens, asked of the server")
    assert "wgs-batch-12 on lab-server" in page.text(".found") and "both ways" in page.text(".found")
    assert page.eval("document.querySelector('.box label + .found + label input').value") == "/Users/sam/argus-sync/wgs-batch-12"
    page.click_text("Choose…", within=".box button")
    page.wait("document.querySelector('.box label + .found + label input').value === '/Users/sam/Desktop/chosen'")
    page.click_text("Connect", within=".box button")
    page.wait("document.querySelectorAll('.card').length === 1 && document.querySelector('#sheet').hidden")
    assert "connect" in page.eval(CALLS)


def test_a_bad_link_says_so_and_cannot_connect(make_page):
    page = make_page("?empty")
    page.click_text("Connect a folder", within="#add")
    page.type("hello")
    page.wait("!document.querySelector('.box .err').hidden", what="the error")
    assert "argus-sync://" in page.text(".box .err")
    assert page.eval("[...document.querySelectorAll('.box button')].find(b => b.textContent === 'Connect').disabled")


def test_a_link_clicked_while_open_opens_the_sheet(make_page):
    page = make_page()
    page.eval("window.__mock.emit('invite', 'argus-sync://pair?url=http%3A%2F%2Fx&key=as_y')")
    page.wait("!!document.querySelector('.found')")


def test_the_menu_pause_and_disconnect(make_page):
    page = make_page()
    page.click_text("Pause", within=".card button")
    page.wait("document.querySelector('.card .chip.warn')?.textContent === 'Paused'")
    page.click(".card .cardhead button")
    page.click_text("Disconnect…", within=".menu button")
    assert "The files stay where they are" in page.text(".box")
    page.click_text("Disconnect", within=".box button")
    page.wait("document.querySelectorAll('.card').length === 1")


def test_italian_and_light(make_page):
    page = make_page(lang="it-IT", dark=False)
    text = page.eval("document.body.textContent")
    for words in ("Collega una cartella", "Aggiornata", "Tieni la mia", "In attesa di conferma", "su lab-server"):
        assert words in text, words
    assert page.eval("getComputedStyle(document.body).backgroundColor") == "rgb(246, 248, 250)"


def test_narrow_window_has_no_sideways_scroll(make_page):
    page = make_page(viewport=PHONE_WIDE)
    assert page.eval("document.documentElement.scrollWidth <= innerWidth"), "the window scrolls sideways"
    assert page.eval("[...document.querySelectorAll('.card')].every(c => c.getBoundingClientRect().right <= innerWidth)")


def test_open_asks_the_app_for_the_folder_by_id(make_page):
    """Not a path: the app opens only what is inside a synced folder (the opener plugin's own
    scope refused every path — "not allowed to open" on a real Mac)."""
    page = make_page()
    page.click_text("Open", within=".card .where button")
    page.click(".sect.alert a.link")
    calls = page.eval("JSON.stringify(window.__mock.calls.filter(c => c[0] === 'open_in'))")
    assert calls == '[["open_in",{"id":"a1","rel":null}],["open_in",{"id":"a1","rel":"tables/summary (conflict from lap684 2026-10-14 1128).xlsx"}]]', calls
    assert "plugin:opener" not in (page.eval("fetch('app.js').then(r => r.text())"))
