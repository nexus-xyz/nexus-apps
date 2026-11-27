#!/usr/bin/env python3
"""The leak gate (see .github/oss-leak-gate/README.md). Standard library only.

  leak_gate.py tree [--summary FILE]   scan every tracked file: gitleaks, semgrep, trufflehog fail; blocklist reports
  leak_gate.py staged                  gitleaks on the staged changes only (the pre-commit hook)

Exit 0 when clean, 1 on a finding that fails, 2 when a tool is missing or fails. Values are never printed.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CONFIG = ROOT / ".github" / "oss-leak-gate"
# The gate's own config and the semgrep rules' test cases (synthetic secrets): the only files the scan skips. Named one
# by one, so a file someone adds to the folder is scanned like any other.
OWN_FILES = {f".github/oss-leak-gate/{name}" for name in
             ("blocklist.toml", "gitleaks.toml", "semgrep.yml", "semgrep.rs", "semgrep.ts")}


def need(*tools: str) -> None:
    missing = [t for t in tools if shutil.which(t) is None]
    if missing:
        print(f"leak gate: install {', '.join(missing)} first (README.md lists the versions)", file=sys.stderr)
        sys.exit(2)


def snapshot(dest: Path) -> None:
    """Copy the tracked files, minus the gate's own config and test cases, into dest."""
    out = subprocess.run(["git", "-C", str(ROOT), "ls-files", "-z"], capture_output=True, check=True).stdout
    for rel in filter(None, out.decode().split("\0")):
        if rel in OWN_FILES:
            continue
        src = ROOT / rel
        if src.is_file() and not src.is_symlink():
            target = dest / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, target)


def run(cmd: list[str], ok: tuple[int, ...] = (0,)) -> str:
    """Runs a scanner. A tool that fails reports its exit code and stderr's last line, never stdout (findings)."""
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode not in ok:
        last = (proc.stderr.strip().splitlines() or ["(no stderr)"])[-1][:300]
        print(f"leak gate: {cmd[0]} exited {proc.returncode}: {last}", file=sys.stderr)
        sys.exit(2)
    return proc.stdout


def gitleaks(source: Path, config: Path, report: Path) -> list[dict]:
    run(["gitleaks", "dir", str(source), "--config", str(config), "--redact", "--no-banner", "--exit-code", "0",
         "--report-format", "json", "--report-path", str(report)])
    return [{"tool": "gitleaks", "rule": f["RuleID"], "file": f["File"], "line": f["StartLine"]}
            for f in json.loads(report.read_text() or "[]")]


def semgrep(source: Path) -> list[dict]:
    out = run(["semgrep", "scan", "--config", str(CONFIG / "semgrep.yml"), "--json", "--quiet", "--metrics=off",
               "--disable-version-check", "--no-git-ignore", str(source)], ok=(0, 1))
    data = json.loads(out or "{}")
    # An error-level problem (a rule that does not load, a config semgrep refuses) means the scan did not happen: fail.
    # Warnings (a file it parsed in part, a rule that timed out on one file) are reported and do not fail.
    problems = data.get("errors", [])
    fatal = [e for e in problems if e.get("level") == "error"]
    if fatal:
        print(f"leak gate: semgrep reported {len(fatal)} errors: {fatal[0].get('type')}", file=sys.stderr)
        sys.exit(2)
    if problems:
        print(f"leak gate: semgrep reported {len(problems)} warnings (partial parses or timeouts)", file=sys.stderr)
    return [{"tool": "semgrep", "rule": r["check_id"].rsplit(".", 1)[-1], "file": r["path"], "line": r["start"]["line"]}
            for r in data.get("results", [])]


def trufflehog(source: Path) -> list[dict]:
    # 183 is trufflehog's "found results" exit code.
    out = run(["trufflehog", "filesystem", str(source), "--only-verified", "--json", "--no-update"], ok=(0, 183))
    found = []
    for line in out.splitlines():
        try:
            f = json.loads(line)
        except json.JSONDecodeError:
            continue
        meta = f.get("SourceMetadata", {}).get("Data", {}).get("Filesystem", {})
        found.append({"tool": "trufflehog", "rule": f.get("DetectorName", "?"), "file": meta.get("file", "?"),
                      "line": meta.get("line", 0)})
    return found


def relative(found: list[dict], source: Path) -> list[dict]:
    for f in found:
        f["file"] = str(Path(f["file"]).resolve().relative_to(source.resolve())) if f["file"] != "?" else "?"
    return found


def table(title: str, found: list[dict]) -> str:
    rows = "".join(f"| {f['tool']} | {f['rule']} | `{f['file']}` | {f['line']} |\n" for f in found)
    return f"### {title}: {len(found)}\n\n" + ("| Tool | Rule | File | Line |\n| --- | --- | --- | --- |\n" + rows if found else "")


def tree(summary: str | None) -> int:
    need("git", "gitleaks", "semgrep", "trufflehog")
    with tempfile.TemporaryDirectory() as tmp:
        source = Path(tmp) / "tree"
        snapshot(source)
        secrets = relative(gitleaks(source, CONFIG / "gitleaks.toml", Path(tmp) / "g.json"), source)
        secrets += relative(semgrep(source), source)
        secrets += relative(trufflehog(source), source)
        refs = relative(gitleaks(source, CONFIG / "blocklist.toml", Path(tmp) / "b.json"), source)
    text = table("Secrets (fail)", secrets) + "\n" + table("Internal references (report only)", refs)
    print(text)
    if summary:
        with open(summary, "a", encoding="utf-8") as out:
            out.write(text + "\n")
    if refs:
        print(f"::warning::{len(refs)} internal references (report only, see the summary)")
    return 1 if secrets else 0


def staged() -> int:
    need("git", "gitleaks")
    run = subprocess.run(
        ["gitleaks", "git", str(ROOT), "--staged", "--pre-commit", "--config", str(CONFIG / "gitleaks.toml"),
         "--redact", "--no-banner"],
    )
    return 1 if run.returncode else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="mode", required=True)
    sub.add_parser("tree").add_argument("--summary", help="append the markdown tables to this file")
    sub.add_parser("staged")
    args = parser.parse_args()
    return tree(args.summary) if args.mode == "tree" else staged()


if __name__ == "__main__":
    sys.exit(main())
