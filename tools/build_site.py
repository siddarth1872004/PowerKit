#!/usr/bin/env python3
"""Build the GitHub Pages site into site/: docs/index.html with the utility list
taken from the README table, so the two never disagree."""
import html
import re
import shutil
from pathlib import Path

root = Path(__file__).resolve().parent.parent
out = root / "site"
row = re.compile(r'^\| <img src="assets/modules/(\w+)\.png"[^|]*\| \*\*(.+?)\*\* \| (.+?) \| (.+?) \|$')


def keycaps(md):
    """Shortcuts in backticks as key caps; `Win+Ctrl+1…9` becomes Win, Ctrl, 1…9."""
    out = []
    for part in re.split(r"(`[^`]+`)", md):
        if part.startswith("`"):
            keys = [f"<kbd>{html.escape(k)}</kbd>" for k in part.strip("`").split("+")]
            out.append(f'<span class="combo">{"+".join(keys)}</span>')
        elif part.strip(" ,"):
            out.append(f"<span>{inline(part.strip(' ,'))}</span>")
    return "".join(out)


def inline(md):
    s = html.escape(md, quote=False)
    s = re.sub(r"`([^`]+)`", r"<code>\1</code>", s)
    return re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", s)


cards = []
for line in (root / "README.md").read_text(encoding="utf-8").splitlines():
    m = row.match(line)
    if m:
        icon, name, what, keys = m.groups()
        keys = "" if keys.strip() in ("—", "-") else f'\n  <div class="keys">{keycaps(keys)}</div>'
        cards.append(
            f'<div class="card">\n  <h3><img src="icons/{icon}.png" alt="">{inline(name)}</h3>\n'
            f"  <p>{inline(what)}</p>{keys}\n</div>"
        )
if not cards:
    raise SystemExit("no utilities found in README.md")

page = (root / "docs" / "index.html").read_text(encoding="utf-8")
for marker in ("<!-- UTILITIES -->", "<!-- VERSION -->", "<!-- COUNT -->"):
    if marker not in page:
        raise SystemExit(f"docs/index.html has no {marker} marker")
version = re.search(r'^version = "([^"]+)"', (root / "Cargo.toml").read_text(encoding="utf-8"), re.M)
if not version:
    raise SystemExit("no version in Cargo.toml")
page = page.replace("<!-- VERSION -->", version.group(1)).replace("<!-- COUNT -->", str(len(cards)))
shutil.rmtree(out, ignore_errors=True)
(out / "icons").mkdir(parents=True)
(out / "index.html").write_text(page.replace("<!-- UTILITIES -->", "\n".join(cards)), encoding="utf-8")
for png in (root / "assets" / "modules").glob("*.png"):
    shutil.copy(png, out / "icons" / png.name)
shutil.copy(root / "apps/desktop/src-tauri/icons/128x128@2x.png", out / "icon.png")
print(f"site/ built with {len(cards)} utilities")
