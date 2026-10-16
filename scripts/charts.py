#!/usr/bin/env python3
"""Render the README charts as light and dark SVGs (docs/assets/charts/*-{light,dark}.svg).

Numbers live here, next to the doc that reports them (docs/comparisons/git-vs-pando.md);
update both together. Run: python3 scripts/charts.py
"""
import os

OUT = os.path.join(os.path.dirname(__file__), "..", "docs", "assets", "charts")
FONT = "ui-sans-serif, -apple-system, 'Segoe UI', Helvetica, Arial, sans-serif"
THEMES = {
    "light": {"text": "#23282D", "muted": "#5B6670", "rule": "#D5DBD7", "pando": "#16324F", "git": "#A9B8B0"},
    "dark": {"text": "#EEF1EC", "muted": "#A3B0A9", "rule": "#33414E", "pando": "#F0B429", "git": "#5E6F7D"},
}

# (title, unit note, rows of (label, value, shown), higher_is_better)
PANELS = [
    (
        "1,000 agents landing on one branch",
        "changes landed per second, higher is better",
        [("Pando", 504, "504 /s"), ("git push + rebase", 0.2, "0.2 /s")],
    ),
    (
        "Clone git/git (82,443 commits)",
        "seconds, lower is better",
        [("Pando", 0.463, "0.46 s"), ("git clone", 19.13, "19.1 s")],
    ),
]


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;")


def panel(x, title, note, rows, t, width=400):
    label_w, bar_x0 = 128, x + 128
    bar_max = width - label_w - 70
    top = max(v for _, v, _ in rows)
    out = [
        f'<text x="{x}" y="22" font-size="15" font-weight="600" fill="{t["text"]}">{esc(title)}</text>',
        f'<text x="{x}" y="42" font-size="12" fill="{t["muted"]}">{esc(note)}</text>',
    ]
    for i, (label, v, shown) in enumerate(rows):
        y = 64 + i * 34
        w = max(bar_max * v / top, 2)
        color = t["pando"] if label == "Pando" else t["git"]
        weight = 600 if label == "Pando" else 400
        out += [
            f'<text x="{x}" y="{y + 15}" font-size="13" font-weight="{weight}" fill="{t["text"]}">{esc(label)}</text>',
            f'<rect x="{bar_x0}" y="{y + 2}" width="{w:.1f}" height="18" rx="3" fill="{color}"/>',
            f'<text x="{bar_x0 + w + 8:.1f}" y="{y + 15}" font-size="13" font-weight="{weight}" fill="{t["text"]}">{esc(shown)}</text>',
        ]
    return out


def render(theme):
    t = THEMES[theme]
    body = []
    for i, (title, note, rows) in enumerate(PANELS):
        body += panel(i * 440, title, note, rows, t)
    body.append(f'<line x1="420" y1="8" x2="420" y2="124" stroke="{t["rule"]}"/>')
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 840 132" width="840" height="132" '
        f'font-family="{FONT}" role="img" aria-label="Pando lands 504 changes per second from 1,000 agents '
        f'where git manages 0.2; Pando clones git/git in 0.46 s where git takes 19.1 s.">\n'
        + "\n".join(body)
        + "\n</svg>\n"
    )


def box(x, y, w, h, title, lines, t, strong=False):
    fill = t["pando"] if strong else "none"
    ink = ("#EEF1EC" if t is THEMES["light"] else "#16324F") if strong else t["text"]
    sub = ink if strong else t["muted"]
    out = [f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="8" fill="{fill}" stroke="{t["pando"] if strong else t["rule"]}" stroke-width="1.5"/>',
           f'<text x="{x + 14}" y="{y + 24}" font-size="14" font-weight="600" fill="{ink}">{esc(title)}</text>']
    for i, line in enumerate(lines):
        out.append(f'<text x="{x + 14}" y="{y + 44 + i * 17}" font-size="12" fill="{sub}">{esc(line)}</text>')
    return out


def arrow(x1, y1, x2, y2, t, label=None, dashed=False):
    dash = ' stroke-dasharray="4 4"' if dashed else ""
    out = [f'<line x1="{x1}" y1="{y1}" x2="{x2 - 6}" y2="{y2}" stroke="{t["muted"]}" stroke-width="1.5"{dash}/>',
           f'<path d="M{x2} {y2} l-7 -4 v8z" fill="{t["muted"]}"/>']
    if label:
        out.append(f'<text x="{(x1 + x2) / 2}" y="{y1 - 7}" font-size="11" fill="{t["muted"]}" text-anchor="middle">{esc(label)}</text>')
    return out


def architecture(theme):
    t = THEMES[theme]
    b = []
    b += box(0, 30, 170, 112, "Clients", ["pando CLI", "git (remote helper)", "AI agents (MCP)", "SDKs: Python, TS, Go"], t)
    b += box(214, 30, 150, 112, "Node daemon", ["one per CI machine", "memory + disk cache", "prefetches main"], t)
    b += box(408, 30, 150, 112, "Regional cache", ["stateless", "read-through", "scale out freely"], t)
    b += box(602, 10, 236, 124, "pandod server", ["creates commits, rebases, lands", "history, blame, diff, search", "permissions, SSO, land checks",
                                                 "every feature behind a flag"], t, strong=True)
    b += box(602, 170, 112, 70, "Metadata", ["SQLite, Postgres,", "CockroachDB"], t)
    b += box(726, 170, 112, 70, "Objects", ["S3, GCS, Azure,", "disk"], t)
    b += arrow(170, 86, 214, 86, t)
    b += arrow(364, 86, 408, 86, t)
    b += arrow(558, 86, 602, 86, t)
    b.append(f'<text x="386" y="20" font-size="11" fill="{t["muted"]}" text-anchor="middle">file contents, cached at every hop</text>')
    # commits, lands and history queries skip the caches
    b.append(f'<path d="M85 142 V206 H586 V118 H595" fill="none" stroke="{t["muted"]}" stroke-width="1.5" stroke-dasharray="4 4"/>')
    b.append(f'<path d="M602 118 l-7 -4 v8z" fill="{t["muted"]}"/>')
    b.append(f'<text x="97" y="198" font-size="11" fill="{t["muted"]}">commit, land, log, blame: small calls straight to the server</text>')
    b.append(f'<line x1="658" y1="134" x2="658" y2="170" stroke="{t["muted"]}" stroke-width="1.5"/>')
    b.append(f'<line x1="782" y1="134" x2="782" y2="170" stroke="{t["muted"]}" stroke-width="1.5"/>')
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 840 244" width="840" height="244" font-family="{FONT}" role="img" '
            f'aria-label="Clients fetch file contents through a node daemon and a regional cache from the pandod server; '
            f'commits, lands and history queries go straight to the server, which stores metadata in SQLite, Postgres or CockroachDB '
            f'and objects in S3, GCS, Azure or disk.">\n' + "\n".join(b) + "\n</svg>\n")


def main():
    os.makedirs(OUT, exist_ok=True)
    for theme in THEMES:
        with open(os.path.join(OUT, f"headline-{theme}.svg"), "w") as f:
            f.write(render(theme))
        with open(os.path.join(OUT, f"architecture-{theme}.svg"), "w") as f:
            f.write(architecture(theme))


if __name__ == "__main__":
    main()
