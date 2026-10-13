"""End-to-end checks for the portfolio against a running server.

Usage:
  python tests/e2e/test_portfolio.py <full_url> <static_url> <db_path> <screenshot_dir>

<full_url>   Rust server serving the site + API
<static_url> plain static host of the same build (no API), like GitHub Pages
"""

import sqlite3
import sys
from pathlib import Path

from playwright.sync_api import Page, expect, sync_playwright

FULL, STATIC, DB, SHOTS = sys.argv[1], sys.argv[2], sys.argv[3], Path(sys.argv[4])
VIEWPORTS = {"desktop": (1440, 900), "mobile": (390, 844)}


def open_page(browser, url: str, size: tuple[int, int]) -> tuple[Page, list[str]]:
    page = browser.new_page(viewport={"width": size[0], "height": size[1]})
    errors: list[str] = []
    page.on("console", lambda m: m.type in ("error", "warning") and errors.append(m.text))
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(url)
    page.wait_for_load_state("networkidle")
    return page, errors


def no_horizontal_scroll(page: Page) -> None:
    overflow = page.evaluate("document.documentElement.scrollWidth - window.innerWidth")
    assert overflow <= 0, f"horizontal overflow of {overflow}px"


def check_full_stack(browser, name: str, size: tuple[int, int]) -> None:
    page, errors = open_page(browser, FULL, size)
    no_horizontal_scroll(page)
    expect(page.get_by_role("heading", level=1)).to_contain_text("grounded")
    page.screenshot(path=SHOTS / f"{name}-top.png")
    page.screenshot(path=SHOTS / f"{name}-full.png", full_page=True)

    # Ask: results cite their source and link to it.
    ask = page.get_by_role("searchbox", name="Search the résumé")
    expect(ask).to_be_visible()
    ask.fill("MCP tenancy")
    first = page.locator("#ask-results a").first
    expect(first).to_contain_text("AutoResolve")
    expect(page.locator("#ask-results mark").first).to_be_visible()
    expect(page.locator("[data-ask-status]")).to_contain_text("cite")
    page.locator(".ask-panel").screenshot(path=SHOTS / f"{name}-ask.png")
    first.click()
    page.wait_for_function("location.hash === '#project-autoresolve'")
    expect(page.locator("#project-autoresolve")).to_be_in_viewport()

    ask.fill("zzzqqq")
    expect(page.locator("[data-ask-status]")).to_contain_text("Nothing here mentions")
    ask.fill('"NEAR( ))')  # FTS syntax must not surface as an error
    expect(page.locator("[data-ask-status]")).not_to_contain_text("wrong")

    # Contact: invalid submit marks fields; valid submit stores the message.
    form = page.locator("[data-contact-form]")
    form.scroll_into_view_if_needed()
    page.get_by_role("button", name="Send message").click()
    expect(page.get_by_label("Name")).to_have_attribute("aria-invalid", "true")
    expect(page.locator("#c-name-err")).to_be_visible()
    expect(page.locator("[data-contact-status]")).to_contain_text("Check the highlighted fields")
    page.get_by_label("Name").fill(f"E2E {name}")
    page.get_by_label("Work email").fill("recruiter@example.com")
    page.get_by_label("Message").fill("Hello from the end-to-end test suite.")
    expect(page.locator("[data-contact-status]")).to_have_text("")
    page.screenshot(path=SHOTS / f"{name}-form.png")
    page.get_by_role("button", name="Send message").click()
    expect(page.locator("[data-contact-status]")).to_contain_text("Sent.")
    expect(page.get_by_label("Name")).to_have_value("")
    with sqlite3.connect(DB) as db:
        (count,) = db.execute("SELECT count(*) FROM messages WHERE name = ?", (f"E2E {name}",)).fetchone()
    assert count == 1, f"expected stored message, found {count}"

    # Keyboard: skip link is the first tab stop.
    page.goto(FULL)
    page.keyboard.press("Tab")
    expect(page.get_by_role("link", name="Skip to content")).to_be_focused()

    assert not errors, f"console errors: {errors}"
    page.close()


def check_static_only(browser, name: str, size: tuple[int, int]) -> None:
    page, errors = open_page(browser, STATIC, size)
    no_horizontal_scroll(page)
    expect(page.get_by_role("searchbox")).to_be_hidden()
    expect(page.locator("[data-contact-form]")).to_be_hidden()
    expect(page.get_by_role("link", name="chiragraicr7@gmail.com")).to_be_visible()
    page.screenshot(path=SHOTS / f"{name}-static.png", full_page=True)
    # The only expected error is the failed health probe (404 on a static host).
    unexpected = [e for e in errors if "404" not in e]
    assert not unexpected, f"console errors: {unexpected}"
    page.close()


def check_headers(browser) -> None:
    page = browser.new_page()
    resp = page.goto(FULL)
    csp = resp.headers.get("content-security-policy", "")
    assert "script-src 'self'" in csp and "frame-ancestors 'none'" in csp, csp
    missing = page.goto(FULL.rstrip("/") + "/does-not-exist")
    assert missing.status == 404
    expect(page.get_by_role("heading", level=1)).to_contain_text("doesn’t exist")
    page.close()


def main() -> None:
    SHOTS.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        try:
            check_headers(browser)
            for name, size in VIEWPORTS.items():
                check_full_stack(browser, name, size)
                check_static_only(browser, name, size)
        finally:
            browser.close()
    print("e2e: all checks passed")


if __name__ == "__main__":
    main()
