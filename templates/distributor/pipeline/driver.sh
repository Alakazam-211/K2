#!/usr/bin/env bash
# The distributor pipeline's driver: builds your brand of K5 from a K5
# release tag. Any CI can call it; .github/workflows/brand-build.yml is the
# reference. Your repo holds your brand folder, distributor.conf, hold.txt,
# hooks/ and this file. No K5 source is copied in: each build fetches K5's
# public repo at one tag.
#
#   driver.sh pick
#       Print "<tag> <commit>" for the K5 tag to build now, or nothing: the
#       newest release tag that is at least first_tag, newer than your last
#       shipped one (your repo's shipped/<tag> tags) and not in hold.txt.
#   driver.sh check <tag> [--sha <commit>]
#       The gates only, steps 1 to 5, on any release tag. No build.
#   driver.sh dry-run <tag> [--sha <commit>] [--out <dir>]
#       Steps 1 to 8 on any release tag, held or old, with no secrets. Its
#       output can't be shipped (its manifest says dry_run = yes).
#   driver.sh build <tag> [--sha <commit>] --out <dir>
#       Steps 1 to 8 for real. Refuses a held tag, a tag not newer than
#       your last shipped one, and a build without your mail sign-in
#       clients. Leaves bin/, SHA256SUMS and manifest.txt in <dir>.
#   driver.sh ship --out <dir>
#       Steps 9 to 11, run apart from the build, with your secrets. Runs
#       no K5 code: only this file and your hooks.
#
# The steps (SECRETS marks the ones that need yours):
#   1 fetch     K5's repo at the tag, into <work>/k5; --sha pins the commit
#   2 tools     the tag carries the brand tools this driver calls
#   3 licence   scripts/brand/licence-gate.sh: the tag's Cargo.lock holds no
#               crate a private white-label build can't ship (today:
#               libgit2-sys, until the gix migration replaces libgit2)
#   4 ceiling   scripts/brand/ceiling-zero-gate.sh: every command is native
#   5 lint      scripts/brand.sh lint on your brand, and --live (lint_live)
#   6 build     K5's Linux release build with K2_BRAND_DIR = your brand
#               folder (SECRETS for a real build: your mail sign-in clients)
#   7 judge     the CLI's output names your brand only (the output judges,
#               with none of K5's debt rows), and every binary carries your
#               brand mark alone (scripts/brand/check-brand-mark.py)
#   8 keys      a report: the build trusts your update key and none of K5's
#   9 check     the output is a real build of your brand, unchanged; with
#               publish = on, step 8's report must pass
#  10 sign      hooks/sign.sh, with your signing key (SECRETS)
#  11 publish   hooks/publish.sh, to your feed (SECRETS); then the tag
#               shipped/<K5 tag> in your repo, for your CI to push
#
# Settings: distributor.conf in your repo's root (DISTRIBUTOR_CONF names
# another). DISTRIBUTOR_ROOT is your repo's root (default: this file's
# folder's parent). --work <dir> (default $RUNNER_TEMP or $TMPDIR, then
# k5-brand-build) holds the K5 checkout and the judges' corpus. Steps 6 to
# 8 and ship run on Linux. Exit 0 done, 1 refused or failed, 2 usage.
set -euo pipefail

ROOT="${DISTRIBUTOR_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CONF="${DISTRIBUTOR_CONF:-$ROOT/distributor.conf}"
STEP="setup"

say() { echo "driver: $*"; }
step() { STEP="$1"; shift; echo ""; echo "── step $STEP: $* ──"; }
refuse() { echo "driver: REFUSED at step $STEP: $*" >&2; exit 1; }
usage() {
    sed -n 's/^#   \(driver\.sh .*\)/usage: \1/p' "$0" >&2
    exit 2
}

# ── settings ──────────────────────────────────────────────────────────

KEYS="upstream tag_regex first_tag brand_dir lint_live target publish feed_url update_pubkey"

load_conf() { # every setting into C_<key>, once; a missing one is refused
    local line k v key
    [ -f "$CONF" ] || refuse "no settings file at $CONF"
    for key in $KEYS; do printf -v "C_$key" '%s' ""; done
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in ''|'#'*) continue ;; esac
        case "$line" in *=*) ;; *) refuse "$CONF: not a key = value line: $line" ;; esac
        k="$(printf '%s' "${line%%=*}" | tr -d ' \t')"
        v="$(printf '%s' "${line#*=}" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
        case " $KEYS " in *" $k "*) ;; *) refuse "$CONF: a key this driver does not know: $k" ;; esac
        printf -v "C_$k" '%s' "$v"
    done < "$CONF"
    for key in $KEYS; do
        v="C_$key"
        [ -n "${!v}" ] || refuse "$CONF has no $key"
    done
    for key in lint_live publish; do
        v="C_$key"
        case "${!v}" in on|off) ;; *) refuse "$key in $CONF must be on or off, not ${!v}" ;; esac
    done
    case "$C_brand_dir" in /*) BRAND="$C_brand_dir" ;; *) BRAND="$ROOT/$C_brand_dir" ;; esac
    [ -f "$BRAND/brand.toml" ] || refuse "no brand.toml in $BRAND"
    BRAND="$(cd "$BRAND" && pwd)"
}

brand_field() { # brand_field <id|product|cli>
    python3 - "$1" "$BRAND/brand.toml" <<'EOF'
import sys, tomllib
with open(sys.argv[2], "rb") as f:
    doc = tomllib.load(f)
print({"id": doc["id"], "product": doc["names"]["product"], "cli": doc["cli"]["name"]}[sys.argv[1]])
EOF
}

# ── versions and tags ─────────────────────────────────────────────────

newer() { # newer <a> <b>: a is a later version than b
    [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$1" ]
}

is_release() { printf '%s\n' "$1" | grep -Eq -- "$C_tag_regex"; }

held() { # held <tag>: 0 when hold.txt holds it, or holds all
    local f="$ROOT/hold.txt" word
    [ -f "$f" ] || return 1
    while read -r word _; do
        case "$word" in ''|'#'*) continue ;; esac
        if [ "$word" = all ] || [ "$word" = "$1" ]; then return 0; fi
    done < "$f"
    return 1
}

last_shipped() { # your newest shipped/<tag>, or nothing
    local tags t
    tags="$(git -C "$ROOT" tag -l 'shipped/*')" || refuse "$ROOT is not a git repo: can't read its shipped/ tags"
    printf '%s\n' "$tags" | sed -n 's#^shipped/##p' | while IFS= read -r t; do
        if is_release "$t"; then printf '%s\n' "$t"; fi
    done | sort -V | tail -n 1
}

upstream_tags() { # "<tag> <commit>" for every release tag upstream
    git ls-remote --tags "$C_upstream" | awk '
        { ref = $2; sub("^refs/tags/", "", ref)
          if (ref ~ /\^\{\}$/) { sub(/\^\{\}$/, "", ref); peeled[ref] = $1 }
          else { direct[ref] = $1 } }
        END { for (t in direct) print t, (t in peeled ? peeled[t] : direct[t]) }' |
        while read -r t c; do
            if is_release "$t"; then printf '%s %s\n' "$t" "$c"; fi
        done | sort -V
}

cmd_pick() {
    local last list best="" t c
    last="$(last_shipped)"
    if held all; then say "hold.txt holds all new tags: nothing to build" >&2; return 0; fi
    list="$(upstream_tags)" || refuse "can't list the tags of $C_upstream"
    while read -r t c; do
        [ -n "$t" ] || continue
        if newer "$C_first_tag" "$t"; then continue; fi
        if [ -n "$last" ] && ! newer "$t" "$last"; then continue; fi
        if held "$t"; then say "$t is held (hold.txt)" >&2; continue; fi
        best="$t $c"
    done <<< "$list"
    if [ -n "$best" ]; then
        printf '%s\n' "$best"
    else
        say "nothing new to build (last shipped: ${last:-none})" >&2
    fi
}

# ── steps 1 to 5: the gates ───────────────────────────────────────────

TOOLS="scripts/brand.sh scripts/brand/lint.py brand/reserved-names.txt
scripts/brand/licence-gate.sh scripts/brand/copyleft-crates.txt
scripts/brand/ceiling-zero-gate.sh scripts/brand/collect-outputs.sh
scripts/brand/output-judges.py scripts/brand/output-keep.txt
scripts/brand/check-brand-mark.py crates/k2-brand/build.rs
crates/k2-release-keys/src/lib.rs"

gates() { # gates <tag> <sha or empty> <dry: yes|no>
    local tag="$1" sha="$2" dry="$3" got f last id
    python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)' \
        || refuse "needs Python 3.11 or newer (the brand tools read TOML)"
    is_release "$tag" || refuse "$tag is not a release tag (tag_regex in $CONF)"
    if [ "$dry" = no ]; then
        if newer "$C_first_tag" "$tag"; then refuse "$tag is older than first_tag $C_first_tag"; fi
        if held "$tag"; then refuse "$tag is held (hold.txt); a dry run still works"; fi
        last="$(last_shipped)"
        if [ -n "$last" ] && ! newer "$tag" "$last"; then
            refuse "$tag is not newer than your last shipped tag $last: a feed never goes back"
        fi
    fi

    step 1 "fetch K5 at $tag"
    rm -rf "$K5"
    git init -q "$K5"
    git -C "$K5" fetch -q --depth 1 "$C_upstream" "refs/tags/$tag:refs/tags/$tag" \
        || refuse "can't fetch tag $tag from $C_upstream"
    got="$(git -C "$K5" rev-parse "refs/tags/$tag^{commit}")"
    if [ -n "$sha" ] && [ "$got" != "$sha" ]; then
        refuse "tag $tag is commit $got, not $sha: the tag moved since it was picked"
    fi
    git -C "$K5" -c advice.detachedHead=false checkout -q "refs/tags/$tag"
    K5_SHA="$got"
    say "K5 $tag is commit $got"

    step 2 "the tag carries the brand tools"
    for f in $TOOLS; do
        [ -e "$K5/$f" ] || refuse "$tag has no $f: it predates brand builds"
    done
    say "all there"

    step 3 "licence: no crate a private white-label build can't ship"
    id="$(brand_field id)"
    bash "$K5/scripts/brand/licence-gate.sh" --brand "$id" "$K5" \
        || refuse "the tag links code a private white-label build can't ship (above)"

    step 4 "ceiling-zero: every command is native"
    bash "$K5/scripts/brand/ceiling-zero-gate.sh" --tree "$K5" \
        || refuse "the tag still hands commands to the shell CLI"

    step 5 "the name policy on $BRAND"
    bash "$K5/scripts/brand.sh" lint "$BRAND" || refuse "your brand fails the name policy"
    if [ "$C_lint_live" = on ]; then
        bash "$K5/scripts/brand.sh" lint "$BRAND" --live || refuse "your brand fails the live name checks"
    else
        say "lint_live = off: the live host and package checks did not run"
    fi
}

# ── steps 6 to 8: build and judge ─────────────────────────────────────

BINS="k2-daemon k2-node k2-root-helper k2-app-engine k5-cli"
MARKED="k2-daemon k2-node k2-app-engine k5-cli"

build_linux() { # build_linux <target> <out> <dry>
    local target="$1" out="$2" dry="$3" dir b channel
    cd "$K5"
    # The Rust the tag pins (rust-toolchain.toml), with this target.
    channel="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"
    [ -n "$channel" ] || refuse "no Rust channel in the tag's rust-toolchain.toml"
    rustup toolchain install "$channel" --profile minimal --target "$target"
    # K5's own Linux release build (.github/workflows/daemon-binaries.yml
    # at the tag), with your brand. The helpers come from the tag.
    if [ "$dry" = no ]; then
        # shellcheck source=/dev/null
        source scripts/require-mail-oauth-build-env.sh
        require_mail_oauth_build_env || refuse "your mail sign-in clients are not set (README, the secrets)"
    else
        say "dry run: built without your mail sign-in clients"
    fi
    # shellcheck source=/dev/null
    source scripts/ci/repro-build-env.sh
    k2_repro_build_env "$K5"
    # shellcheck source=/dev/null
    source scripts/linux-ptyd-embed.sh
    k2_ptyd_build_for_embed "$target"
    k2_export_build_for_embed "$target"
    k2_deskd_build_for_embed "$target"
    cargo build --locked --release --target "$target" --bin k2-daemon --bin k2-node
    cargo build --locked --release --target "$target" -p k2-root-helper
    cargo build --locked --release --target "$target" -p k2-app-engine
    cargo build --locked --release --target "$target" -p k5-cli --bin k5-cli
    dir="${CARGO_TARGET_DIR:-target}/$target/release"
    mkdir -p "$out/bin"
    for b in $BINS; do cp "$dir/$b" "$out/bin/$b"; done
    assert_daemon_embeds_ptyd "$out/bin/k2-daemon"
    assert_daemon_embeds_export "$out/bin/k2-daemon"
    assert_daemon_embeds_deskd "$out/bin/k2-daemon"
    for b in K2_ROOT_HELPER_TEST_KEYS_ENABLED:k2-root-helper K2_NODE_TEST_KEYS_ENABLED:k2-node \
        K2_APP_ENGINE_TEST_KEYS_ENABLED:k2-app-engine; do
        if grep -a -q "${b%%:*}" "$out/bin/${b#*:}"; then refuse "${b#*:} was built with test keys"; fi
    done
    python3 -I scripts/lib/check-k5-cli.py "$out/bin/k5-cli" cli/k2 "linux-${target%%-*}"
    if [ "$dry" = no ]; then assert_daemon_oauth_not_placeholder "$out/bin/k2-daemon"; fi
    cd - >/dev/null
}

k5_update_keys() { # the update keys K5's own builds trust, from the tag
    sed -n '/BEGIN UPDATER_KEYS/,/END UPDATER_KEYS/p' "$K5/crates/k2-release-keys/src/lib.rs" |
        sed -n 's/.*public_key_b64: *"\([A-Za-z0-9+\/=]*\)".*/\1/p'
}

keys_check() { # keys_check <out>: 0 when the build trusts your update key alone
    local out="$1" bad=0 b k
    [ -s "$out/k5-update-keys.txt" ] || { echo "keys: no list of K5's update keys in $out" >&2; return 1; }
    for b in k2-daemon k5-cli; do
        while IFS= read -r k; do
            if [ -n "$k" ] && grep -a -q -F -- "$k" "$out/bin/$b"; then
                echo "keys: $b trusts K5's update key $k" >&2
                bad=1
            fi
        done < "$out/k5-update-keys.txt"
    done
    if ! grep -a -q -F -- "$C_update_pubkey" "$out/bin/k2-daemon"; then
        echo "keys: k2-daemon does not carry your update key (update_pubkey)" >&2
        bad=1
    fi
    return "$bad"
}

cmd_build() { # cmd_build <tag> <sha> <out> <dry>
    local tag="$1" sha="$2" out="$3" dry="$4" id judge
    [ -n "$out" ] || usage
    # The folder is emptied: only a new one, or an earlier output folder.
    if [ -d "$out" ] && [ -n "$(ls -A "$out")" ] && [ ! -f "$out/manifest.txt" ]; then
        refuse "$out is not empty and is not an earlier output folder (no manifest.txt)"
    fi
    gates "$tag" "$sha" "$dry"
    id="$(brand_field id)"
    rm -rf "$out"
    mkdir -p "$out"
    out="$(cd "$out" && pwd)"

    step 6 "build K5 $tag as $id ($C_target)"
    export K2_BRAND_DIR="$BRAND"
    build_linux "$C_target" "$out" "$dry"

    step 7 "the output judges and the brand mark"
    judge="$WORK/judge"
    rm -rf "$judge"
    mkdir -p "$judge"
    bash "$K5/scripts/brand/collect-outputs.sh" "$out/bin/k5-cli" "$BRAND" "$judge/corpus"
    if [ -s "$judge/fell-through.txt" ]; then
        refuse "forms fell through to the shell CLI: $(tr '\n' ' ' < "$judge/fell-through.txt")"
    fi
    # K5's debt rows are lines K5 still has to fix: your build carries none.
    grep -v '^debt' "$K5/scripts/brand/output-keep.txt" > "$judge/keep.txt"
    python3 "$K5/scripts/brand/output-judges.py" --brand "$BRAND" --keep "$judge/keep.txt" "$judge/corpus" \
        || refuse "the build prints a name that is not yours (above)"
    # shellcheck disable=SC2086
    (cd "$out/bin" && python3 "$K5/scripts/brand/check-brand-mark.py" --brand "$id" $MARKED) \
        || refuse "a binary is not marked as your brand alone"

    step 8 "which update keys the build trusts"
    k5_update_keys > "$out/k5-update-keys.txt"
    [ -s "$out/k5-update-keys.txt" ] || refuse "found no update keys in the tag's crates/k2-release-keys/src/lib.rs"
    if keys_check "$out"; then
        say "the build trusts your update key and none of K5's: it can be published"
    else
        say "the build can't update from your feed yet (above): ship refuses to publish it"
    fi

    # shellcheck disable=SC2086
    (cd "$out/bin" && sha256sum $BINS) > "$out/SHA256SUMS"
    {
        echo "brand_id = $id"
        echo "command = $(brand_field cli)"
        echo "product = $(brand_field product)"
        echo "k5_tag = $tag"
        echo "k5_commit = $K5_SHA"
        echo "version = ${tag#v}"
        echo "target = $C_target"
        echo "dry_run = $dry"
    } > "$out/manifest.txt"
    echo ""
    if [ "$dry" = yes ]; then
        say "dry run of $tag: every step up to signing passed; nothing signed or published ($out)"
    else
        say "built $tag as $id in $out; next: driver.sh ship --out $out"
    fi
}

# ── steps 9 to 11: ship (your secrets, no K5 code) ────────────────────

manifest() { sed -n "s/^$1 = //p" "$OUT/manifest.txt"; }

cmd_ship() {
    local tag
    [ -n "$OUT" ] && [ -f "$OUT/manifest.txt" ] || refuse "no manifest.txt in ${OUT:-(no --out)}"
    step 9 "the output is a real build of your brand, unchanged"
    [ "$(manifest dry_run)" = no ] || refuse "a dry run's output is never shipped"
    (cd "$OUT/bin" && sha256sum -c --quiet ../SHA256SUMS) || refuse "the binaries changed since the build"
    [ "$(manifest brand_id)" = "$(brand_field id)" ] || refuse "built for $(manifest brand_id), not your brand"
    if [ "$C_publish" = on ]; then
        keys_check "$OUT" || refuse "the build does not trust your update key alone: nothing signed or published"
    fi

    step 10 "sign with your key (hooks/sign.sh)"
    bash "$ROOT/hooks/sign.sh" "$OUT" "$CONF"

    if [ "$C_publish" = off ]; then
        say "publish = off: the signed files are in $OUT; nothing published, no shipped tag"
        return 0
    fi
    step 11 "publish to your feed (hooks/publish.sh)"
    bash "$ROOT/hooks/publish.sh" "$OUT" "$CONF"
    tag="$(manifest k5_tag)"
    git -C "$ROOT" tag -a "shipped/$tag" \
        -m "K5 $tag ($(manifest k5_commit)) shipped as $(manifest brand_id) $(manifest version)"
    say "shipped $tag; push the tag shipped/$tag to your repo"
}

# ── the command line ──────────────────────────────────────────────────

[ $# -ge 1 ] || usage
cmd="$1"
shift
TAG="" SHA="" OUT="" WORK="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/k5-brand-build"
case "$cmd" in
    check|build|dry-run) [ $# -ge 1 ] || usage; TAG="$1"; shift ;;
    pick|ship) ;;
    *) usage ;;
esac
while [ $# -gt 0 ]; do
    case "$1" in
        --sha) [ $# -ge 2 ] || usage; SHA="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || usage; OUT="$2"; shift 2 ;;
        --work) [ $# -ge 2 ] || usage; WORK="$2"; shift 2 ;;
        *) usage ;;
    esac
done
load_conf
mkdir -p "$WORK"
WORK="$(cd "$WORK" && pwd)"
K5="$WORK/k5"
K5_SHA=""

case "$cmd" in
    pick) cmd_pick ;;
    check) gates "$TAG" "$SHA" yes; echo ""; say "$TAG passes the gates for $(brand_field id)" ;;
    dry-run) cmd_build "$TAG" "$SHA" "${OUT:-$WORK/out}" yes ;;
    build) cmd_build "$TAG" "$SHA" "$OUT" no ;;
    ship) cmd_ship ;;
esac
