#!/bin/sh
# Pack-template check: a type pack must never depend on or import a CRDT engine.
# The kit owns the engine (Rust) and the JS client + editor-binding adapters; packs bind to
# the kit's primitives. Run from a pack repo's root: ../k2-multiplayer-base/scripts/pack-checks/engine-free.sh
#
# Fails on:
#  - a Cargo dependency (any target, direct) on an engine crate in any pack crate;
#  - a JS dependency on an engine package in any package.json (outside node_modules);
#  - a source import/use of an engine in .rs/.ts/.tsx/.js/.mjs/.cjs files.
set -eu
ROOT="${1:-.}"
cd "$ROOT"
python3 -I - <<'PY'
import json, os, re, subprocess, sys
RUST = {"yrs", "y-sync", "yrs-warp", "loro", "loro-internal", "automerge", "autosurgeon", "diamond-types"}
JS = {"yjs", "y-protocols", "y-codemirror.next", "y-prosemirror", "y-monaco", "y-websocket", "lib0",
      "loro-crdt", "loro-codemirror", "@automerge/automerge", "@automerge/automerge-repo"}
SKIP_DIRS = {"node_modules", "target", ".git"}
bad = []

meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"],
                                 check=True, capture_output=True, text=True).stdout)
for p in meta["packages"]:
    for d in p["dependencies"]:
        if d["name"] in RUST:
            bad.append(f"{p['name']}: Cargo dependency on engine crate `{d['name']}`")

src_re = re.compile(r"""(?:^\s*(?:pub\s+)?use\s+|extern\s+crate\s+|\byrs::|\bloro::|\bautomerge::)(yrs|loro|automerge)\b|(?:from\s+|require\(\s*|import\s*\(\s*|import\s+)['"]((?:@automerge/)?[\w.\-]+)""", re.M)
for dirpath, dirnames, files in os.walk("."):
    dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
    for f in files:
        path = os.path.join(dirpath, f)
        if f == "package.json":
            pkg = json.load(open(path))
            for section in ("dependencies", "devDependencies", "peerDependencies", "optionalDependencies"):
                for name in pkg.get(section, {}):
                    if name in JS:
                        bad.append(f"{path}: {section} on engine package `{name}`")
        elif f.endswith((".rs", ".ts", ".tsx", ".js", ".mjs", ".cjs")):
            text = open(path, encoding="utf-8", errors="replace").read()
            for m in src_re.finditer(text):
                name = m.group(1) or m.group(2)
                if name in RUST or name in JS:
                    line = text.count("\n", 0, m.start()) + 1
                    bad.append(f"{path}:{line}: imports engine `{name}`")

if bad:
    print("A type pack must not touch the CRDT engine (use the kit's primitives and JS client):")
    print("\n".join("  " + b for b in bad))
    sys.exit(1)
print("engine-free: ok")
PY
