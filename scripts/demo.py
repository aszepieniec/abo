#!/usr/bin/env python3
"""The README's animation, docs/demo.gif: the README's example, run.

    scripts/demo.py

builds abo, runs the commands below on new books in a temporary directory,
and records what a terminal would show as an asciinema cast, the typing
included, as docs/demo.cast. agg (cargo install --git
https://github.com/asciinema/agg) draws the cast as the GIF. The day, the
commits' dates and the git identity are fixed, so the two change only when
what abo prints changes: run this again after such a change, and commit
both. The cast is text, so its diff says what changed.
"""

import json
import os
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path

COMMANDS = [
    "abo init Acme Ltd --currency EUR --vat none",
    "abo account add Equity:Capital",
    "abo post --desc 'Capital paid in' Assets:Bank=1000 Equity:Capital=-1000",
    "abo post --desc Hosting Expenses:General=20 Assets:Bank=-20",
    "abo pl --period 2026",
    "abo log",
]

DAY = "2026-03-02"
COLS, ROWS = 120, 28
TYPE, AFTER_ENTER, PAUSE, END = 0.04, 0.4, 1.6, 4.0  # seconds

ROOT = Path(__file__).resolve().parent.parent
CAST, GIF = ROOT / "docs" / "demo.cast", ROOT / "docs" / "demo.gif"


def main():
    subprocess.run(["cargo", "build", "--release", "--quiet"], cwd=ROOT, check=True)
    abo = ROOT / "target" / "release" / "abo"
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        books = tmp / "acme"
        books.mkdir()
        gitconfig = tmp / "gitconfig"
        gitconfig.write_text("[user]\n\tname = Ada\n\temail = ada@example.invalid\n")
        env = dict(
            os.environ,
            ABO_TODAY=DAY,
            GIT_CONFIG_GLOBAL=str(gitconfig),
            GIT_AUTHOR_DATE=f"{DAY}T09:30:00Z",
            GIT_COMMITTER_DATE=f"{DAY}T09:30:00Z",
            TZ="UTC",
            COLUMNS=str(COLS),
        )
        t, events = 0.5, []

        def show(text, gap):
            nonlocal t
            t += gap
            events.append([round(t, 3), "o", text])

        for command in COMMANDS:
            show("\x1b[1;32m$\x1b[0m ", PAUSE if events else 0)
            for ch in command:
                show(ch, TYPE)
            argv = [str(abo)] + shlex.split(command)[1:]
            run = subprocess.run(argv, cwd=books, env=env, capture_output=True, text=True)
            # Where the books are is the reader's own directory, not this one.
            printed = (run.stdout + run.stderr).replace(str(books), "~/acme")
            show("\r\n" + printed.replace("\n", "\r\n"), AFTER_ENTER)
        show("", END)

    header = {"version": 2, "width": COLS, "height": ROWS}
    CAST.parent.mkdir(exist_ok=True)
    CAST.write_text("\n".join(json.dumps(e) for e in [header] + events) + "\n")
    subprocess.run(["agg", "--quiet", "--idle-time-limit", "5", str(CAST), str(GIF)], check=True)
    print(f"wrote {CAST.relative_to(ROOT)} and {GIF.relative_to(ROOT)}")


if __name__ == "__main__":
    sys.exit(main())
