#!/usr/bin/env bash
# Tests for the distributor pipeline template (templates/distributor/):
# its files read right, its example brand passes the name policy, it names
# no real distributor, and its driver picks, holds and refuses as the
# README says. Offline: scratch git repos on disk, no network, no build,
# no CLI, no cargo.
#
# The workflow is parsed as YAML when python3 has the yaml module; without
# it that one check says it did not run. A private list of real names a
# public file must never hold can be given in BRAND_PRIVATE_NAMES_FILE (one
# name per line, kept outside this repo); without it the test says the
# list was not given and checks the placeholder rules only.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
TPL="$HERE/templates/distributor"
D="$TPL/pipeline/driver.sh"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
export PYTHONDONTWRITEBYTECODE=1
unset DISTRIBUTOR_ROOT DISTRIBUTOR_CONF RUNNER_TEMP || true
pass=0
ok() { pass=$((pass + 1)); }
die() { echo "FAIL: $*" >&2; exit 1; }

# ── the files ──────────────────────────────────────────────────────────
for f in README.md brand/brand.toml distributor.conf hold.txt pipeline/driver.sh \
    hooks/sign.sh hooks/publish.sh .github/workflows/brand-build.yml; do
    [ -f "$TPL/$f" ] || die "the template has no $f"
done
ok
for f in pipeline/driver.sh hooks/sign.sh hooks/publish.sh; do
    bash -n "$TPL/$f" || die "$f does not parse (bash -n)"
done
ok

WF="$TPL/.github/workflows/brand-build.yml"
if python3 -c 'import yaml' 2>/dev/null; then
    python3 - "$WF" <<'EOF' || die "the workflow: see above"
import sys, yaml
doc = yaml.safe_load(open(sys.argv[1]))
jobs = doc["jobs"]
assert set(jobs) == {"pick", "build", "ship"}, f"jobs: {sorted(jobs)}"
assert jobs["ship"].get("environment") == "release", "ship runs in the release environment"
assert jobs["ship"]["needs"] == ["pick", "build"], "ship needs pick and build"
# YAML 1.1 reads the key "on" as True.
on = doc.get("on", doc.get(True))
assert "schedule" in on and "workflow_dispatch" in on, "runs on a schedule and by hand"
for job in ("pick", "build"):
    assert "secrets." not in yaml.safe_dump(jobs[job]).replace("secrets.MAIL_", ""), \
        f"{job} names a secret other than the mail sign-in clients"
EOF
    echo "yaml: the workflow parses, three jobs, ship behind the release environment"
else
    echo "yaml: NOT RUN (python3 has no yaml module); the workflow's text checks below still ran"
fi
ok

# Every action is pinned to a full commit sha, with its version beside it.
unpinned="$(grep -E '^[[:space:]]*-?[[:space:]]*uses:' "$WF" | grep -vE 'uses: [A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+@[0-9a-f]{40} # v[0-9]+(\.[0-9]+)*$' || true)"
[ -z "$unpinned" ] || die "an action not pinned to a commit sha with its version: $unpinned"
[ "$(grep -cE 'uses:' "$WF")" -ge 5 ] || die "the workflow's actions were not all seen"
ok

# Signing and feed secrets live only in the ship job, and every secret the
# workflow names is in the README's table.
build_part="$(sed -n '/^  build:/,/^  ship:/p' "$WF")"
if printf '%s\n' "$build_part" | grep -qE 'secrets\.(UPDATE|FEED)_'; then
    die "the build job names a signing or feed secret"
fi
ok
for s in $(grep -oE 'secrets\.[A-Z0-9_]+' "$WF" | sed 's/secrets\.//' | sort -u); do
    grep -qF "\`$s\`" "$TPL/README.md" || die "the workflow's secret $s is not in the README's table"
    grep -qF "$s" "$TPL/hooks/sign.sh" "$TPL/hooks/publish.sh" "$WF" || die "$s"
done
ok

# ── the example brand ─────────────────────────────────────────────────
rc=0
out="$(bash "$HERE/scripts/brand.sh" lint "$TPL/brand" 2>&1)" || rc=$?
[ "$rc" = 0 ] || die "the example brand fails the name policy: $out"
[ "$out" = "brand lint: globex: command gy, product 'Globex Studio': ok (the written lists)" ] \
    || die "the example brand's lint line: $out"
ok

# ── no real distributor's name ────────────────────────────────────────
# scan <folder>: the template's text holds no private name and teaches no
# two-character command but the example's and ours.
scan() {
    local dir="$1" bad=0 w hits
    hits="$(grep -rhoE '`[a-z][a-z0-9]`' "$dir" | sort -u | tr -d '`' | grep -vxE 'gy|k2|k5' || true)"
    if [ -n "$hits" ]; then
        echo "a two-character command that is not the example's: $(echo $hits)" >&2
        bad=1
    fi
    if [ -n "${BRAND_PRIVATE_NAMES_FILE:-}" ]; then
        while IFS= read -r w; do
            w="$(printf '%s' "$w" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
            [ -n "$w" ] || continue
            if grep -rqiwF -- "$w" "$dir"; then
                echo "a private name is in $(grep -rliwF -- "$w" "$dir" | head -n 1)" >&2
                bad=1
            fi
        done < "$BRAND_PRIVATE_NAMES_FILE"
    fi
    return "$bad"
}
if [ -n "${BRAND_PRIVATE_NAMES_FILE:-}" ]; then
    [ -s "$BRAND_PRIVATE_NAMES_FILE" ] || die "BRAND_PRIVATE_NAMES_FILE names an empty or missing file"
    echo "names: checked against the private list ($(grep -c . "$BRAND_PRIVATE_NAMES_FILE") names)"
else
    echo "names: the private list was NOT given (BRAND_PRIVATE_NAMES_FILE); the placeholder rules ran"
fi
cp "$HERE/tests/garden/distributor-template.sh" "$T/self.sh"
scan "$TPL" || die "the template names something it must not (above)"
scan "$T/self.sh" || die "this test names something it must not (above)"
ok
# Bite: a planted command fails the scan.
cp -R "$TPL" "$T/bite"
printf 'Run %sq9%s to say hi.\n' '`' '`' >> "$T/bite/README.md"
if scan "$T/bite" 2>/dev/null; then die "a planted two-character command passed the scan"; fi
ok

# ── the driver, on scratch repos ──────────────────────────────────────
# A stand-in K5 repo: the brand tools of this tree, a ceiling file, and
# release tags with and without them.
U="$T/k5"
CEIL="crates/k5-cli/tests/data/native_bar_ceiling.tsv"
git init -q "$U"
git -C "$U" config user.email t@example.invalid
git -C "$U" config user.name t
echo "a K5 before brand builds" > "$U/README"
git -C "$U" add -A && git -C "$U" commit -qm old && git -C "$U" tag v0.46.9
mkdir -p "$U/scripts" "$U/crates/k2-brand" "$U/crates/k2-release-keys/src" "$U/$(dirname "$CEIL")"
cp -R "$HERE/scripts/brand" "$U/scripts/brand"
cp "$HERE/scripts/brand.sh" "$U/scripts/brand.sh"
cp -R "$HERE/brand" "$U/brand"
echo "// stand-in" > "$U/crates/k2-brand/build.rs"
echo "// stand-in" > "$U/crates/k2-release-keys/src/lib.rs"
ceiling() { # ceiling <fell> <H count> [<row>]
    { printf '# %s of 5117 probes fall through: V 0, H %s, F 0, E 0, P 0\n' "$1" "$2"
      printf 'class\tfamily\tform\n'
      if [ $# -gt 2 ]; then printf '%s\n' "$3"; fi; } > "$U/$CEIL"
}
release() { git -C "$U" add -A && git -C "$U" commit -qm "$1" && git -C "$U" tag -a "$1" -m "$1"; }
ceiling 1 1 "$(printf 'H\tmail\tmail status --help')"
release v0.47.0
ceiling 0 0
release v0.47.1
echo "fix" >> "$U/README"
release v0.47.2
git -C "$U" tag vp8-decoder-v0.1.2
sha() { git -C "$U" rev-parse "$1^{commit}"; }

# A stand-in distributor repo: the template, with K5 on disk.
R="$T/dist"
cp -R "$TPL" "$R"
git init -q "$R"
git -C "$R" config user.email t@example.invalid
git -C "$R" config user.name t
sed -e "s#^upstream = .*#upstream = file://$U#" -e 's/^lint_live = .*/lint_live = off/' \
    "$TPL/distributor.conf" > "$R/distributor.conf"
git -C "$R" add -A && git -C "$R" commit -qm start
drv() { rc=0; out="$(bash "$R/pipeline/driver.sh" "$@" --work "$T/work" 2>&1)" || rc=$?; }
pick() { rc=0; out="$(bash "$R/pipeline/driver.sh" pick --work "$T/work" 2>/dev/null)" || rc=$?; }

pick; [ "$rc" = 0 ] && [ "$out" = "v0.47.2 $(sha v0.47.2)" ] || die "pick: the newest release, peeled to its commit: $out"; ok
echo "v0.47.2  # our QA" >> "$R/hold.txt"
pick; [ "$out" = "v0.47.1 $(sha v0.47.1)" ] || die "pick: a held tag is passed over: $out"; ok
echo "all" >> "$R/hold.txt"
pick; [ "$rc" = 0 ] && [ -z "$out" ] || die "pick: all holds everything: $out"; ok
sed -i.bak '/^all$/d' "$R/hold.txt" && rm -f "$R/hold.txt.bak"
git -C "$R" tag -a shipped/v0.47.1 -m shipped
pick; [ "$rc" = 0 ] && [ -z "$out" ] || die "pick: nothing newer than shipped and not held: $out"; ok
sed -i.bak '/^v0.47.2/d' "$R/hold.txt" && rm -f "$R/hold.txt.bak"
pick; [ "$out" = "v0.47.2 $(sha v0.47.2)" ] || die "pick: the tag once its hold is taken out: $out"; ok

drv check v0.46.9
[ "$rc" = 1 ] && [[ "$out" == *"REFUSED at step 2: v0.46.9 has no scripts/brand.sh: it predates brand builds"* ]] \
    || die "check: an old tag: $out"; ok
drv check v0.47.0
[ "$rc" = 1 ] && [[ "$out" == *"ceiling-zero: REFUSED"* ]] && [[ "$out" == *"REFUSED at step 3"* ]] \
    || die "check: a tag that falls through: $out"; ok
drv check v0.47.1
[ "$rc" = 0 ] && [[ "$out" == *"brand lint: globex: command gy"* ]] && [[ "$out" == *"lint_live = off"* ]] \
    && [[ "$out" == *"v0.47.1 passes the gates for globex"* ]] || die "check: a ready tag: $out"; ok
drv check v0.47.1 --sha "$(sha v0.47.2)"
[ "$rc" = 1 ] && [[ "$out" == *"REFUSED at step 1: tag v0.47.1 is commit $(sha v0.47.1), not"* ]] \
    || die "check: a tag that moved: $out"; ok
drv check vp8-decoder-v0.1.2
[ "$rc" = 1 ] && [[ "$out" == *"is not a release tag"* ]] || die "check: not a release tag: $out"; ok

echo "v0.47.2" >> "$R/hold.txt"
rm -rf "$T/work/k5"
drv build v0.47.2 --out "$T/out"
[ "$rc" = 1 ] && [[ "$out" == *"v0.47.2 is held (hold.txt)"* ]] && [ ! -e "$T/work/k5" ] \
    || die "build: a held tag, refused before the fetch: $out"; ok
sed -i.bak '/^v0.47.2/d' "$R/hold.txt" && rm -f "$R/hold.txt.bak"
drv build v0.47.1 --out "$T/out"
[ "$rc" = 1 ] && [[ "$out" == *"not newer than your last shipped tag v0.47.1"* ]] || die "build: a shipped tag: $out"; ok
mkdir -p "$T/busy" && echo keep > "$T/busy/notes"
drv build v0.47.2 --out "$T/busy"
[ "$rc" = 1 ] && [[ "$out" == *"is not empty and is not an earlier output folder"* ]] && [ -f "$T/busy/notes" ] \
    || die "build: a folder that is not an output folder is left alone: $out"; ok

# ship: a dry run's output is refused; a changed binary is refused; with
# publish on, a build that trusts K5's update key is refused before signing.
O="$T/ship"
mkdir -p "$O/bin"
k5key="RWRstandinK5key0000000000000000000000000000000000000000000"
printf 'daemon %s\n' "$k5key" > "$O/bin/k2-daemon"
printf 'cli\n' > "$O/bin/k5-cli"
echo "$k5key" > "$O/k5-update-keys.txt"
(cd "$O/bin" && sha256sum k2-daemon k5-cli) > "$O/SHA256SUMS"
manifest() { printf 'brand_id = globex\nk5_tag = v0.47.2\nversion = 0.47.2\ndry_run = %s\n' "$1" > "$O/manifest.txt"; }
manifest yes
drv ship --out "$O"
[ "$rc" = 1 ] && [[ "$out" == *"REFUSED at step 8: a dry run's output is never shipped"* ]] || die "ship: a dry run: $out"; ok
manifest no
sed -i.bak 's/^publish = .*/publish = on/' "$R/distributor.conf" && rm -f "$R/distributor.conf.bak"
drv ship --out "$O"
[ "$rc" = 1 ] && [[ "$out" == *"k2-daemon trusts K5's update key"* ]] && [[ "$out" == *"does not carry your update key"* ]] \
    && [[ "$out" == *"nothing signed or published"* ]] || die "ship: a build that trusts K5's key: $out"; ok
echo "changed" >> "$O/bin/k5-cli"
drv ship --out "$O"
[ "$rc" = 1 ] && [[ "$out" == *"the binaries changed since the build"* ]] || die "ship: a changed binary: $out"; ok

# Settings: an unknown key and a missing one are refused.
echo "colour = blue" >> "$R/distributor.conf"
drv check v0.47.1
[ "$rc" = 1 ] && [[ "$out" == *"a key this driver does not know: colour"* ]] || die "settings: an unknown key: $out"; ok
grep -v '^colour\|^feed_url' "$R/distributor.conf" > "$R/c2" && mv "$R/c2" "$R/distributor.conf"
drv check v0.47.1
[ "$rc" = 1 ] && [[ "$out" == *"has no feed_url"* ]] || die "settings: a missing key: $out"; ok

echo "distributor-template: $pass checks passed"
