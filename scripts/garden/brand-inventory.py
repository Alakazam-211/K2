#!/usr/bin/env python3
"""Count where the names K2, k2 and k2.dev are, by kind and by area.

    python3 scripts/garden/brand-inventory.py            a table (markdown)
    python3 scripts/garden/brand-inventory.py --json     the same numbers as JSON
    python3 scripts/garden/brand-inventory.py --lines KIND [AREA]
                                                         the counted lines of one cell

For the K2 to K5 work: what people and agents read, and the names that
other programs hold. It reads the tracked files (`git ls-files`), so it
shows the tree as committed. Nothing is written and nothing is checked
against a saved number: every lane adds and removes such lines, so a saved
count would fail all of them. Run it when the numbers are wanted.

A line is counted once per kind. "code" means the line is not a comment
(`//`, `///`, `#`, `*`, `<!--`) and, in Rust, is above the file's first
`#[cfg(test)] mod`. Test files are their own area, whatever their folder.

Kinds:
  product-name   the word K2 on its own (K2, "K2 Connect", K2.app; not K2_X,
                 K2Error, X-K2-Header or k2) on a code line
  cli-command    `k2 <word>` (text that tells someone to run the CLI) on a
                 code line
  zone           `k2.dev` on a code line
  env-name       distinct K2_* names (a count of names, not of lines)
  header         distinct X-K2-* header names (names, any case)
  token-prefix   distinct k2xxx_ prefixes of tokens and ids (names)
  rdns-id        distinct dev.k2.* ids: bundle ids, launchd labels, keychain
                 services (names)
  home-path      `~/.k2` or `$HOME/.k2` or join(".k2 on a code line

Not counted anywhere: vendor/, third_party/, lock files, the built cli/k2
(cli/src is counted), generated files, tests/golden, WHATS_NEW.md (history).
"""
import json
import re
import subprocess
import sys

SKIP_PREFIX = ("vendor/", "third_party/", "tests/golden/", "sdk/generated/", "sdk/server/dist/", "sdk/server/src/generated/")
SKIP_EXACT = {"cli/k2", "WHATS_NEW.md", "src/shared/cli-catalog.generated.json"}
SKIP_SUFFIX = (".lock", ".png", ".icns", ".ico", ".jpg", ".woff2", ".pdf", ".dat", ".snap", ".svg")
TEXT_SUFFIX = (".rs", ".ts", ".tsx", ".js", ".mjs", ".sh", ".py", ".md", ".json", ".toml", ".yml", ".yaml", ".html", ".css",
               ".plist", ".nsh", ".desktop", ".service", ".install", ".txt", ".fish", ".tpl", ".conf")

AREAS = ["rust", "cli", "renderer", "tauri-config", "sdk", "catalog", "scripts-ci", "packaging", "docs", "tests", "other"]
LINE_KINDS = ["product-name", "cli-command", "zone", "home-path"]
NAME_KINDS = ["env-name", "header", "token-prefix", "rdns-id"]

RE = {
    "product-name": re.compile(r"(?<![A-Za-z0-9_])(?<![Xx]-)K2(?![A-Za-z0-9_])"),
    "cli-command": re.compile(r"(?<![\w./-])k2 [a-z]"),
    "zone": re.compile(r"k2\.dev\b"),
    "home-path": re.compile(r"(~|\$HOME|\$\{HOME\})/\.k2(?![a-z0-9])|join\([\"']\.k2(?![a-z0-9])"),
    "env-name": re.compile(r"(?<![A-Za-z0-9_])K2_[A-Z][A-Z0-9_]*[A-Z0-9]"),
    "header": re.compile(r"(?i)\bx-k2-[a-z0-9]+(?:-[a-z0-9]+)*"),
    "token-prefix": re.compile(r"(?<![A-Za-z0-9_])k2[a-z]{1,6}_(?=[^a-z_]|$)"),
    "rdns-id": re.compile(r"\bdev\.k2\.[a-z0-9]+(?:[.-][a-z0-9]+)*"),
}
TEST_NAME = re.compile(r"(^tests\.rs$|_tests?\.rs$|^test_.*\.rs$|\.test\.(ts|tsx|js|mjs)$|\.spec\.(ts|tsx)$)")
TEST_MOD = re.compile(r"\s*(pub(\([a-z]+\))? )?mod \w+")
COMMENT = ("//", "#", "*", "/*", "<!--", "{/*")


def area_of(path):
    name = path.rsplit("/", 1)[-1]
    parts = path.split("/")
    if parts[0] == "tests" or TEST_NAME.search(name) or any(p in ("tests", "__tests__", "fixtures", "benches", "fuzz") for p in parts[:-1]):
        return "tests"
    if path == "crates/k2-core/src/contract/catalog.json":
        return "catalog"
    if path.startswith("crates/") or path.startswith("src-tauri/src/"):
        return "rust" if path.endswith(".rs") else "other"
    if path.startswith("src-tauri/"):
        return "tauri-config"
    if path.startswith("cli/"):
        return "cli"
    if path.startswith("src/"):
        return "renderer"
    if path.startswith("sdk/"):
        return "sdk"
    if path.startswith("scripts/") or path.startswith(".github/"):
        return "scripts-ci"
    if path.startswith("packaging/"):
        return "packaging"
    if path.startswith("docs/") or path.endswith(".md"):
        return "docs"
    return "other"


def code_lines(path, text):
    """(line number, line) for the lines that count as code."""
    lines = text.split("\n")
    is_rust = path.endswith(".rs")
    is_prose = path.endswith(".md") or path.endswith(".json") or path.endswith(".txt")
    for i, line in enumerate(lines):
        s = line.strip()
        if is_rust and s.startswith("#[cfg(test)]") and i + 1 < len(lines) and TEST_MOD.match(lines[i + 1]):
            return
        if not is_prose and s.startswith(COMMENT) and not s.startswith("#["):
            continue
        yield i + 1, line


def tracked_files():
    out = subprocess.run(["git", "ls-files", "-z"], check=True, capture_output=True).stdout.decode("utf-8", "replace")
    for path in out.split("\0"):
        if not path or path in SKIP_EXACT or path.startswith(SKIP_PREFIX) or path.endswith(SKIP_SUFFIX):
            continue
        if path.endswith(TEXT_SUFFIX) or "." not in path.rsplit("/", 1)[-1]:
            yield path


def collect(want_lines=None):
    counts = {k: {a: 0 for a in AREAS} for k in LINE_KINDS}
    names = {k: {a: set() for a in AREAS} for k in NAME_KINDS}
    shown = []
    for path in tracked_files():
        try:
            with open(path, encoding="utf-8") as f:
                text = f.read()
        except (UnicodeDecodeError, IsADirectoryError, FileNotFoundError):
            continue  # a binary file, a submodule folder, or a file deleted in the work tree
        if "k2" not in text and "K2" not in text:
            continue
        area = area_of(path)
        for n, line in code_lines(path, text):
            for kind in LINE_KINDS:
                if RE[kind].search(line):
                    counts[kind][area] += 1
                    if want_lines and want_lines[0] == kind and want_lines[1] in (None, area):
                        shown.append(f"{path}:{n}: {line.strip()[:160]}")
            for kind in NAME_KINDS:
                for m in RE[kind].findall(line):
                    names[kind][area].add(m.lower() if kind == "header" else m)
    return counts, names, shown


def main(argv):
    want_lines = None
    as_json = False
    if argv[:1] == ["--lines"] and len(argv) in (2, 3) and argv[1] in LINE_KINDS and (len(argv) == 2 or argv[2] in AREAS):
        want_lines = (argv[1], argv[2] if len(argv) == 3 else None)
    elif argv == ["--json"]:
        as_json = True
    elif argv:
        sys.exit("brand-inventory: usage: brand-inventory.py [--json | --lines KIND [AREA]]\n"
                 f"  KIND: {', '.join(LINE_KINDS)}\n  AREA: {', '.join(AREAS)}")
    counts, names, shown = collect(want_lines)
    if want_lines:
        print("\n".join(shown))
        return 0
    all_names = {k: sorted(set().union(*names[k].values())) for k in NAME_KINDS}
    if as_json:
        print(json.dumps({
            "lines": counts,
            "names": {k: {"all": len(all_names[k]), **{a: len(names[k][a]) for a in AREAS}} for k in NAME_KINDS},
        }, indent=2, sort_keys=True))
        return 0
    head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], check=True, capture_output=True).stdout.decode().strip()
    print(f"K2 names at {head}: code lines per kind and area (see the script's head for what is counted)\n")
    print("| kind | " + " | ".join(AREAS) + " | all |")
    print("|---|" + "---:|" * (len(AREAS) + 1))
    for kind in LINE_KINDS:
        row = counts[kind]
        print(f"| {kind} | " + " | ".join(str(row[a]) for a in AREAS) + f" | {sum(row.values())} |")
    print("\nDistinct names per kind and area (the last column is distinct across all areas)\n")
    print("| kind | " + " | ".join(AREAS) + " | all |")
    print("|---|" + "---:|" * (len(AREAS) + 1))
    for kind in NAME_KINDS:
        print(f"| {kind} | " + " | ".join(str(len(names[kind][a])) for a in AREAS) + f" | {len(all_names[kind])} |")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
