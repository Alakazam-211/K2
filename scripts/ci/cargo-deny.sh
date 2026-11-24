#!/usr/bin/env bash
# cargo-deny.sh — the app-wide cargo-deny gate: the root deny.toml against
# every crate K2 ships, on the real Cargo.lock, and against every other
# Cargo.lock in the repo that K2 owns (SIDE below).
#
#   scripts/ci/cargo-deny.sh install <dir>   put the pinned cargo-deny at <dir>/bin/cargo-deny
#   scripts/ci/cargo-deny.sh offline         no cargo: the git pins, the shipped list and
#                                            the lock list
#   scripts/ci/cargo-deny.sh check [<cargo-deny>]
#                                            offline, then advisories bans licenses sources
#                                            for each shipped crate and each SIDE tree
#                                            (default: cargo-deny on PATH)
#   scripts/ci/cargo-deny.sh ci <dir>        install <dir>, then check with it (checks.sh audit)
#   scripts/ci/cargo-deny.sh stale <logdir> [<deny.toml>]
#                                            fail if a licence exception or an advisory ignore
#                                            is unused in every tree's log in <logdir>
#                                            (check runs it itself)
#
# Scoping: one run per shipped crate (`--manifest-path <crate>/Cargo.toml`
# makes that crate the only root of the graph), each with its default
# features, which are the features the release builds use. Never
# --all-features: the test-only features (test-keys, test-hostile,
# provider-fakes, sew-test-pins, xvfb-tests...) are not shipped. `--locked`:
# Cargo.lock is read, never updated. Every run reads the root deny.toml
# (--config), never a crate's own deny.toml.
#
# Every Cargo.lock in the repo is either checked or named with the reason it
# isn't. The root Cargo.lock is the shipped runs above. A SIDE tree has its
# own lock outside the shipped graph: the app template's check app, and the
# patched third_party copies the root [patch] points at (the root lock
# covers the code K2 ships from them; their own locks serve building them
# alone). check copies each SIDE tree out of the repo, because third_party
# sits inside the root workspace's folder and cargo there would read the
# root lock, then runs the same four checks on it, --locked, with the root
# deny.toml. LOCKS_EXPLAINED names, by path pattern, the locks not checked
# here and why; offline prints each one with its reason.
#
# `offline` checks, with no cargo:
#   - every git dependency in every Cargo.toml (vendored ones too) is pinned
#     by `rev = "<40-hex sha>"`, with no branch or tag, and every git source
#     in Cargo.lock carries that sha;
#   - every Cargo.lock in the repo (git ls-files, untracked ones too) is
#     the root lock, a SIDE tree's or matches LOCKS_EXPLAINED, and every
#     SIDE tree still has its lock;
#   - every crate the release builds name (`-p <crate>` in
#     scripts/macos-daemon-helper-app.sh, scripts/lib/build-binaries.sh,
#     scripts/ci/build-linux-bins.sh, scripts/linux-ptyd-embed.sh and
#     scripts/windows/build-nsis-release.ps1)
#     is in SHIPPED below, so a new shipped crate can't skip the gate.
#     tests/packaging/cargo-deny-policy.sh tests these, and the policy itself.
#
# After the runs, check fails if a [[licenses.exceptions]] entry or an
# [advisories] ignore was unused in all of the shipped trees (cargo-deny's
# license-exception-not-encountered or advisory-not-detected warning in
# every tree's output): a stale entry is removed, not kept. One tree not
# using it is only that warning.
#
# check needs the internet (the RustSec database and crates.io).
# scripts/ci/checks.sh audit runs install, then check.
#
# A finding: fix the dependency (a lockfile bump in range, a feature off, a
# replacement). A lock bump here re-points unrelated crates' windows-sys
# (osv-scanner.toml's header says why): that re-pointing is its own gated
# commit, never folded into a fix. A licence off the allowlist, a new git source or a new
# exception goes to the Kessel Steward; an advisory K2 can't fix yet gets an
# entry in osv-scanner.toml (reason, review date) and in deny.toml's ignore
# list (scripts/ci/audit-ignores.py checks both). Never --all-features, and
# never a crate's own deny.toml.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CARGO_DENY_VERSION="0.20.2"

# The crates K2 ships, as <manifest dir>:<package>. src-tauri is the app
# (`k2`); the rest are the daemon and the workers K2.app, the Linux release
# and the Windows build carry. k2-signer runs on the release Mac (`_k2sign`).
SHIPPED=(
    "src-tauri:k2"
    "crates/k2-daemon:k2-daemon"
    "crates/k2-ptyd:k2-ptyd"
    "crates/k2-export:k2-export"
    "crates/k2-deckrender:k2-deckrender"
    "crates/k2-scenerender:k2-scenerender"
    "crates/k2-node:k2-node"
    "crates/k2-menubar:k2-menubar"
    "crates/k2-deskd:k2-deskd"
    "crates/k2-root-helper:k2-root-helper"
    "crates/k2-app-engine:k2-app-engine"
    "crates/k5-cli:k5-cli"
    "crates/k2-signer:k2-signer"
)

# The K2-owned trees with their own Cargo.lock, as <lock dir>[|<copy dir>]:
# check copies <copy dir> (default: the lock dir) out of the repo and runs
# cargo-deny on <lock dir>/Cargo.toml there. The app template's build
# script wants its page next to src-tauri, so the whole app is copied.
SIDE=(
    "tests/app-template/tauri-ci-check/src-tauri|tests/app-template/tauri-ci-check"
    "third_party/objc2"
    "third_party/sew-proto"
    "third_party/tao"
    "third_party/tauri-runtime-wry"
    "third_party/wry"
)

# The locks not checked here, as <path pattern>|<why> (fnmatch, on the path
# from the repo root).
LOCKS_EXPLAINED=(
    "vendor/*|another team's code, vendored as they publish it; its owners check it"
)

offline() {
    local root="${1:-$ROOT}"
    local names=()
    local s
    for s in "${SHIPPED[@]}"; do names+=("${s#*:}"); done
    python3 - "$root" "${names[*]}" "$(printf '%s\n' "${SIDE[@]}")" "$(printf '%s\n' "${LOCKS_EXPLAINED[@]}")" <<'PY'
import fnmatch, pathlib, re, subprocess, sys, tomllib

root = pathlib.Path(sys.argv[1])
shipped = set(sys.argv[2].split())
side = [l.split("|")[0] for l in sys.argv[3].splitlines() if l]
explained = [tuple(l.split("|", 1)) for l in sys.argv[4].splitlines() if l]
SHA = re.compile(r"^[0-9a-f]{40}$")
errs = []
skip = {"target", "node_modules", ".git"}


def deps_tables(doc):
    """Every table of dependency specs in a manifest."""
    for key in ("dependencies", "dev-dependencies", "build-dependencies"):
        if isinstance(doc.get(key), dict):
            yield key, doc[key]
    ws = doc.get("workspace", {})
    if isinstance(ws.get("dependencies"), dict):
        yield "workspace.dependencies", ws["dependencies"]
    for tname, t in (doc.get("target") or {}).items():
        for key in ("dependencies", "dev-dependencies", "build-dependencies"):
            if isinstance(t.get(key), dict):
                yield f"target.{tname}.{key}", t[key]
    for src, t in (doc.get("patch") or {}).items():
        if isinstance(t, dict):
            yield f"patch.{src}", t
    if isinstance(doc.get("replace"), dict):
        yield "replace", doc["replace"]


manifests = 0
git_deps = 0
for p in sorted(root.rglob("Cargo.toml")):
    rel = p.relative_to(root)
    if any(part in skip for part in rel.parts):
        continue
    try:
        doc = tomllib.loads(p.read_text())
    except tomllib.TOMLDecodeError as e:
        errs.append(f"{rel}: does not parse: {e}")
        continue
    manifests += 1
    for where, table in deps_tables(doc):
        for name, spec in table.items():
            if not isinstance(spec, dict) or "git" not in spec:
                continue
            git_deps += 1
            rev = spec.get("rev")
            if not isinstance(rev, str) or not SHA.match(rev):
                errs.append(f"{rel} [{where}] {name}: git dependency not pinned by rev = <full 40-hex sha> (rev = {rev!r})")
            for bad in ("branch", "tag"):
                if bad in spec:
                    errs.append(f"{rel} [{where}] {name}: git dependency names a {bad}; pin it by rev only")

lock = root / "Cargo.lock"
lock_git = 0
if lock.exists():
    for pkg in tomllib.loads(lock.read_text()).get("package", []):
        src = pkg.get("source", "")
        if not src.startswith("git+"):
            continue
        lock_git += 1
        m = re.match(r"^git\+[^?#]+\?rev=([0-9a-f]{40})#([0-9a-f]{40})$", src)
        if not m or m.group(1) != m.group(2):
            errs.append(f"Cargo.lock {pkg.get('name')} {pkg.get('version')}: git source not locked to its full rev ({src})")

# Every Cargo.lock: from git in a checkout (tracked, plus untracked ones
# not ignored, so a new lock fails before it is committed), else every
# Cargo.lock under root (a test fixture).
top = subprocess.run(["git", "-C", str(root), "rev-parse", "--show-toplevel"], capture_output=True, text=True)
if top.returncode == 0 and pathlib.Path(top.stdout.strip()).resolve() == root.resolve():
    ls = subprocess.run(["git", "-C", str(root), "ls-files", "-co", "--exclude-standard", "--", "*Cargo.lock"],
                        capture_output=True, text=True, check=True)
    locks = sorted({l for l in ls.stdout.splitlines() if pathlib.PurePosixPath(l).name == "Cargo.lock" and (root / l).exists()})
else:
    locks = sorted(str(p.relative_to(root)) for p in root.rglob("Cargo.lock")
                   if not any(part in skip for part in p.relative_to(root).parts))
if "Cargo.lock" not in locks:
    errs.append("Cargo.lock: the root lock is missing (the shipped runs read it)")
side_seen = 0
skipped = []
for l in locks:
    if l == "Cargo.lock":
        continue
    d = str(pathlib.PurePosixPath(l).parent)
    if d in side:
        side_seen += 1
        continue
    why = next((w for pat, w in explained if fnmatch.fnmatch(l, pat)), None)
    if why is None:
        errs.append(f"{l}: no check covers this lock: add its tree to SIDE in scripts/ci/cargo-deny.sh, or name it in LOCKS_EXPLAINED with the reason")
    else:
        skipped.append((l, why))
for d in side:
    if f"{d}/Cargo.lock" not in locks:
        errs.append(f"SIDE lists {d}, but {d}/Cargo.lock is gone: remove it from SIDE in scripts/ci/cargo-deny.sh")
for l, why in skipped:
    print(f"cargo-deny offline: not checked here: {l}: {why}")

named = set()
for script in ("scripts/macos-daemon-helper-app.sh", "scripts/lib/build-binaries.sh", "scripts/ci/build-linux-bins.sh",
               "scripts/linux-ptyd-embed.sh", "scripts/windows/build-nsis-release.ps1"):
    f = root / script
    if not f.exists():
        errs.append(f"{script}: missing (the shipped-crate list is read from it)")
        continue
    for m in re.finditer(r"(?:^|\s)-p\s+([A-Za-z0-9_-]+)", f.read_text()):
        named.add((m.group(1), script))
for name, script in sorted(named):
    if name not in shipped:
        errs.append(f"{script} builds -p {name} for release, but it is not in SHIPPED (scripts/ci/cargo-deny.sh): add it")

for e in errs:
    print(f"cargo-deny offline: {e}", file=sys.stderr)
if errs:
    sys.exit(1)
print(f"cargo-deny offline: {manifests} manifests, {git_deps} git dependencies and {lock_git} locked git sources, all pinned by full sha; {len(shipped)} shipped crates cover every release -p")
print(f"cargo-deny offline: {len(locks)} Cargo.lock files: the root lock, {side_seen} SIDE trees checked here, {len(skipped)} named with the reason they are not")
PY
}

install() {
    local dir="${1:?usage: cargo-deny.sh install <dir>}"
    if [ -x "$dir/bin/cargo-deny" ] && "$dir/bin/cargo-deny" --version | grep -qx "cargo-deny $CARGO_DENY_VERSION"; then
        echo "cargo-deny $CARGO_DENY_VERSION already at $dir/bin/cargo-deny"
        return 0
    fi
    cargo install cargo-deny --locked --version "=$CARGO_DENY_VERSION" --root "$dir"
}

check() {
    local deny="${1:-cargo-deny}"
    local s dir pkg rc=0 failed=() logs
    logs="$(mktemp -d "${TMPDIR:-/tmp}/k2-cargo-deny.XXXXXX")"
    offline
    "$deny" --version
    for s in "${SHIPPED[@]}"; do
        dir="${s%%:*}" pkg="${s#*:}"
        echo ""
        echo "── cargo deny: $pkg ($dir, default features) ──"
        # cargo-deny writes its findings to stderr: kept in order with the
        # header above, and kept per tree for the stale-entry check
        # (pipefail: the status is cargo-deny's). Each run fetches the
        # RustSec database (a no-op fetch after the first).
        if ! "$deny" --locked --manifest-path "$ROOT/$dir/Cargo.toml" --config "$ROOT/deny.toml" \
                check advisories bans licenses sources 2>&1 | tee "$logs/$pkg.log"; then
            failed+=("$pkg")
            rc=1
        fi
    done
    local side_dir lockdir copy
    side_dir="$(mktemp -d "${TMPDIR:-/tmp}/k2-cargo-deny-side.XXXXXX")"
    for s in "${SIDE[@]}"; do
        lockdir="${s%%|*}" copy="${s#*|}"
        [ "$copy" = "$s" ] && copy="$lockdir"
        echo ""
        echo "── cargo deny: $lockdir (its own Cargo.lock, on a copy outside the repo) ──"
        mkdir -p "$side_dir/$copy"
        cp -R "$ROOT/$copy/." "$side_dir/$copy/"
        if ! "$deny" --locked --manifest-path "$side_dir/$lockdir/Cargo.toml" --config "$ROOT/deny.toml" \
                check advisories bans licenses sources 2>&1 | tee "$logs/side-${lockdir//\//_}.log"; then
            failed+=("$lockdir")
            rc=1
        fi
    done
    rm -rf "$side_dir"
    echo ""
    if ! stale "$logs" "$ROOT/deny.toml"; then
        failed+=("stale exceptions or ignores")
        rc=1
    fi
    rm -rf "$logs"
    if [ "$rc" != 0 ]; then
        echo "cargo-deny: FAILED for ${failed[*]}" >&2
        return 1
    fi
    echo "cargo-deny: ${#SHIPPED[@]} shipped crates and ${#SIDE[@]} SIDE trees pass advisories, bans, licenses and sources"
}

# stale <logdir> <deny.toml>: <logdir> holds one cargo-deny output per
# shipped tree (<package>.log). Fails when a licence exception's crate is
# named by a license-exception-not-encountered warning, or an advisory
# ignore's id by an advisory-not-detected warning, in every log.
stale() {
    local names=() s
    for s in "${SHIPPED[@]}"; do names+=("${s#*:}"); done
    python3 - "$1" "$2" "${names[*]}" <<'PY'
import pathlib, re, sys, tomllib

logdir, deny, names = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3].split()
policy = tomllib.load(open(deny, "rb"))
exceptions = [e["crate"] for e in policy.get("licenses", {}).get("exceptions", [])]
ignores = [e["id"] if isinstance(e, dict) else e for e in policy.get("advisories", {}).get("ignore", [])]
ansi = re.compile(r"\x1b\[[0-9;]*m")
# warning code -> what it quotes from deny.toml (`crate = "<name>"`, `id = "<id>"`).
KINDS = {
    "license-exception-not-encountered": re.compile(r'crate = "([^"]+)"'),
    "advisory-not-detected": re.compile(r'id = "([^"]+)"'),
}
unused = {}
errs = []
for n in names:
    f = logdir / f"{n}.log"
    if not f.exists():
        errs.append(f"no cargo-deny output for {n} ({f})")
        continue
    lines = [ansi.sub("", l) for l in f.read_text().splitlines()]
    hit = {k: set() for k in KINDS}
    for i, line in enumerate(lines):
        for kind, quoted in KINDS.items():
            if f"[{kind}]" not in line:
                continue
            for later in lines[i + 1:i + 6]:
                m = quoted.search(later)
                if m:
                    hit[kind].add(m.group(1))
                    break
    unused[n] = hit
if not errs:
    for crate in exceptions:
        if all(crate in unused[n]["license-exception-not-encountered"] for n in names):
            errs.append(f'the licence exception for "{crate}" is unused in all {len(names)} shipped trees: remove it from {pathlib.Path(deny).name}')
    for vid in ignores:
        if all(vid in unused[n]["advisory-not-detected"] for n in names):
            errs.append(f"the advisory ignore {vid} matches none of the {len(names)} shipped trees: remove it from {pathlib.Path(deny).name} (and its osv-scanner.toml entry if the lock no longer has it)")
for e in errs:
    print(f"cargo-deny stale: {e}", file=sys.stderr)
if errs:
    sys.exit(1)
print(f"cargo-deny stale: each of the {len(exceptions)} licence exceptions and {len(ignores)} advisory ignores is used by at least one of the {len(names)} shipped trees")
PY
}

case "${1:-}" in
    install) shift; install "$@" ;;
    offline) shift; offline "$@" ;;
    check) shift; check "$@" ;;
    stale) shift; stale "${1:?usage: cargo-deny.sh stale <logdir> [<deny.toml>]}" "${2:-$ROOT/deny.toml}" ;;
    ci) shift; install "${1:?usage: cargo-deny.sh ci <dir>}"; check "$1/bin/cargo-deny" ;;
    *) echo "usage: scripts/ci/cargo-deny.sh install <dir> | offline [<root>] | check [<cargo-deny>] | stale <logdir> [<deny.toml>] | ci <dir>" >&2; exit 2 ;;
esac
