#!/usr/bin/env bash
# k2sign-s3b.sh — prd-signing-user-v1 S3b acceptance: the signing user signs
# a K2 Compute-built x86 daemon only on node-signed receipts it checks
# itself. Test mode (macOS, throwaway keys, a fake GitHub API on 127.0.0.1,
# P-256 node keys made with /usr/bin/openssl, a real git repo for the
# commits). No K2SIGN_TEST_EVIDENCE: every signature below passed the real
# compute evidence code.
#   bash tests/release/k2sign-s3b.sh
source "$(dirname "$0")/k2sign_fixture.sh"
ready
OPENSSL=/usr/bin/openssl
REPOP="/repos/Alakazam-211/K2"
NOW0=1800000000
export K2SIGN_TEST_NOW="$NOW0"
ASSETS=(K2.app.tar.gz k2-daemon-linux-aarch64 k2-daemon-linux-x86_64 k2-daemon-macos-aarch64)
CTL="$(printf 'c7%.0s' $(seq 32))"
NOOVER='{}'
PY_CR="$REPO/scripts/ci/compute_release.py"

# ── a real git repo: base B, the release tree TT, the preview commit C
# (what compute built, before Step 8.9) and the tag commit T (same tree) ──
G="$tmp/git"
mkdir -p "$G"
git -C "$G" init -q
git -C "$G" -c user.name=t -c user.email=t@example.com commit -q --allow-empty -m base
printf '{"version": "0.0.0"}\n' > "$G/package.json"
git -C "$G" add package.json
git -C "$G" -c user.name=t -c user.email=t@example.com commit -q -m "base with a package"
B="$(git -C "$G" rev-parse HEAD)"
printf '{"version": "0.47.1"}\n' > "$G/package.json"
git -C "$G" add package.json
TT="$(git -C "$G" write-tree)"
C="$(git -C "$G" -c user.name=t -c user.email=t@example.com commit-tree "$TT" -p "$B" -m "release preview (pre-tag builds only)")"
T="$(GIT_COMMITTER_DATE="2026-11-06T10:00:00Z" git -C "$G" -c user.name=t -c user.email=t@example.com commit-tree "$TT" -p "$B" -m "chore(release): v0.47.1")"
printf 'other\n' > "$G/other.txt"; git -C "$G" add other.txt
TT2="$(git -C "$G" write-tree)"
C_BADPARENT="$(git -C "$G" -c user.name=t -c user.email=t@example.com commit-tree "$TT" -p "$T" -m "preview on another parent")"
C_BADTREE="$(git -C "$G" -c user.name=t -c user.email=t@example.com commit-tree "$TT2" -p "$B" -m "preview of another tree")"
[ "$C" != "$T" ] || { echo "FAIL: the preview and tag commits must differ" >&2; exit 1; }

release_routes() { # <version>: GitHub after Step 8.9: v<x> = T (tree TT), parent B, release files only, B on main
    local v="$1"
    route "$REPOP/git/ref/tags/v$v" "{\"object\": {\"type\": \"commit\", \"sha\": \"$T\"}}"
    route "$REPOP/commits/$T" "{\"sha\": \"$T\", \"commit\": {\"tree\": {\"sha\": \"$TT\"}}, \"parents\": [{\"sha\": \"$B\"}]}"
    route "$REPOP/compare/$B...$T" '{"status": "ahead", "ahead_by": 1, "files": [{"filename": "package.json"}]}'
    route "$REPOP/compare/$B...main" '{"status": "ahead"}'
    route "$REPOP/contents/rust-toolchain.toml?ref=$T" "{\"encoding\": \"base64\", \"content\": \"$TOOLCHAIN_B64\"}"
}
# The tag's rust-toolchain.toml, as GitHub's contents API returns it.
TOOLCHAIN_B64="$(printf '[toolchain]\nchannel = "1.99.1"\ncomponents = ["rustfmt", "clippy"]\n' | base64 | tr -d '\n')"
open_session() { # <version> assets…
    local v="$1"; shift
    "$K" challenge "$v" "$B" "$@" > "$tmp/c.json" 2>"$tmp/c.err" || { fail "challenge $v" "$(cat "$tmp/c.err")"; return 1; }
    approve_with "$tmp/keys/approver.key" "$tmp/c.json" "$tmp/c.der"
    "$K" approve "$(nonce_of "$tmp/c.json")" < "$tmp/c.der" >/dev/null 2>"$tmp/c.err" || fail "approve $v" "$(cat "$tmp/c.err")"
}

# ── node keys (P-256, like k2-node's): build-a and build-b get pinned, rogue never ──
for n in build-a build-b rogue; do
    "$OPENSSL" ecparam -name prime256v1 -genkey -noout -out "$tmp/keys/$n.key" 2>/dev/null
    "$OPENSSL" ec -in "$tmp/keys/$n.key" -pubout -out "$tmp/keys/$n.pem" 2>/dev/null
done
fp_of() { "$OPENSSL" ec -in "$tmp/keys/$1.key" -pubout -outform DER 2>/dev/null | shasum -a 256 | awk '{print $1}'; }
FP_A="$(fp_of build-a)"; FP_B="$(fp_of build-b)"; FP_R="$(fp_of rogue)"

# mkset <version> <dir>: the files one release build makes (the daemon is a
# real-shaped ELF the signer's check-daemon-asset.py accepts).
mkset() {
    local v="$1" d="$2"
    mkdir -p "$d"
    python3 -I - "$d" "$v" <<'PY'
import hashlib, os, struct, sys
d, version = sys.argv[1], sys.argv[2]
def elf(machine, size):
    h = bytearray(64)
    h[0:4] = b"\x7fELF"; h[4] = 2; h[5] = 1; h[6] = 1
    struct.pack_into("<H", h, 18, machine)
    struct.pack_into("<Q", h, 0x28, size - 2 * 64)
    struct.pack_into("<HH", h, 0x3A, 64, 2)
    return bytes(h) + os.urandom(size - 64)
for arch, mach in (("x86_64", 62), ("aarch64", 183)):
    ptyd = elf(mach, 8192)
    body = elf(mach, 4096) + ptyd + hashlib.sha256(ptyd).hexdigest().encode()
    body += b"\0K2_DAEMON_BUILD v=%s os=linux arch=%s;" % (version.encode(), arch.encode())
    body += b"\0" + os.environ["K2_GMAIL_CLIENT_ID"].encode() + b"\0" + os.environ["K2_GMAIL_CLIENT_SECRET"].encode()
    p = os.path.join(d, "k2-daemon-linux-" + arch)
    open(p, "wb").write(body)
    open(p + ".sha256", "w").write("%s  %s\n" % (hashlib.sha256(body).hexdigest(), os.path.basename(p)))
    open(os.path.join(d, "k2-app-engine-linux-" + arch), "wb").write(b"engine-" + arch.encode() + os.urandom(64))
for n in ("k2-root-helper-linux-x86_64", "k2-node-linux-x86_64"):
    open(os.path.join(d, n), "wb").write(n.encode() + os.urandom(64))
PY
}
export K2_GMAIL_CLIENT_ID="test-client-1234.apps.googleusercontent.com" K2_GMAIL_CLIENT_SECRET="GOCSPX-test-secret-5678" K2_REQUIRE_MICROSOFT_OAUTH=0
unset K2_MICROSOFT_CLIENT_ID || true

# mkrcpt <out receipt.json> <signing node> <files dir> [overrides JSON]:
# what `k2 compute receipt <job> --json` prints for one release build job,
# signed with the node's key over the k2 receipt domain bytes (openssl).
# Overrides: any receipt field; "src.<f>"; "drop_art"/"set_art" {name: sha};
# "fp" (claim another node's fp); "tamper" (edit the body after signing);
# "no_body" (an older daemon: only the parsed receipt). The plan (what the
# route returns since the S3b review, H1) follows the receipt; "plan.<k>"
# sets a plan field (null drops it), "plan_tamper" edits the plan after the
# digest went into the receipt, "no_plan" leaves it out.
mkrcpt() {
    python3 -I - "$1" "$tmp/keys/$2.key" "$3" "${4:-$NOOVER}" "$C" "$TT" "$CTL" "$((NOW0 + 60))" "$((NOW0 + 1800))" <<'PY'
import base64, hashlib, json, os, struct, subprocess, sys
out, key, files, over, commit, tree, ctl, started, ended = sys.argv[1:10]
over = json.loads(over)
der = subprocess.run(["/usr/bin/openssl", "ec", "-in", key, "-pubout", "-outform", "DER"],
                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=True).stdout
def canonical(v):
    if isinstance(v, dict):
        return "{" + ",".join(json.dumps(k, ensure_ascii=False) + ":" + canonical(v[k]) for k in sorted(v)) + "}"
    if isinstance(v, list):
        return "[" + ",".join(canonical(x) for x in v) + "]"
    return json.dumps(v, ensure_ascii=False)
arts = []
for n in sorted(os.listdir(files)):
    p = os.path.join(files, n)
    arts.append({"name": "dist/" + n, "sha256": hashlib.sha256(open(p, "rb").read()).hexdigest(), "bytes": os.path.getsize(p)})
for n in over.pop("drop_art", []):
    arts = [a for a in arts if a["name"] != "dist/" + n]
for n, sha in over.pop("set_art", {}).items():
    for a in arts:
        if a["name"] == "dist/" + n:
            a["sha256"] = sha
job = hashlib.sha256(os.urandom(16)).hexdigest()[:32]
r = {"v": 1, "job_id": job, "attempt": 1, "generation": 1, "plan_digest": "sha256:" + "ab" * 32,
     "node_fp": hashlib.sha256(der).hexdigest(), "controller_fp": ctl, "workspace_id": "ws-release",
     "argv_sha256": hashlib.sha256(canonical(["bash", "scripts/ci/build-linux-bins.sh", "x86_64"]).encode()).hexdigest(),
     "env_names": ["BUILD_COMMIT", "BUILD_TREE", "CARGO_HOME", "CI", "HOME", "K2_COMPUTE_JOB", "K2_NODE_OS", "PATH"],
     "secret_env": ["BUILD_GMAIL_CLIENT_ID", "BUILD_GMAIL_CLIENT_SECRET"],
     "src": {"commit": commit, "tree": tree, "dirty_sha256": None, "slot": None, "warmth": "private"},
     "tools": {"rustc": "1.99.1"}, "tools_used": {"rustc": "1.99.1"}, "boot_id": "boot-1",
     "started_at": int(started), "ended_at": int(ended), "state": "done", "exit": {"code": 0, "signal": None},
     "log": {"sha256": "ef" * 32, "bytes": 12, "truncated": False}, "artifacts": arts}
if "fp" in over:
    r["node_fp"] = over.pop("fp")
tamper = over.pop("tamper", False)
no_body = over.pop("no_body", False)
plan_tamper = over.pop("plan_tamper", None)
no_plan = over.pop("no_plan", False)
plan_over = {k[5:]: over.pop(k) for k in [k for k in over if k.startswith("plan.")]}
for k, v in over.items():
    if k.startswith("src."):
        r["src"][k[4:]] = v
    else:
        r[k] = v
plan = {"job_id": r["job_id"], "node_id": "node-1", "workspace_id": "ws-release", "workspace_label": "release",
        "requested_by": "release", "argv": ["bash", "scripts/ci/build-linux-bins.sh", "x86_64"],
        "env": {"BUILD_COMMIT": r["src"]["commit"], "BUILD_TREE": r["src"]["tree"]}, "cwd": None,
        "limits": {"max_secs": 7200, "cpu_millis": 4000}, "exclusive": False, "created_at": int(started) - 30,
        "src": {"project_key": "pk-release", "remote_url": None, "commit": r["src"]["commit"], "dirty": None, "slots": 1},
        "needs": ["rust@1.99.1", "cmake@3", "sys:pkg-config", "sys:zlib1g-dev"], "artifacts": ["dist/*"],
        "packages": ["pkg-config", "zlib1g-dev"], "secret_env": list(r["secret_env"]), "tier": "ci"}
for k, v in plan_over.items():
    if v is None:
        plan.pop(k, None)
    else:
        plan[k] = v
if "plan_digest" not in over:
    r["plan_digest"] = "sha256:" + hashlib.sha256(canonical(plan).encode()).hexdigest()
if plan_tamper:
    plan.update(plan_tamper)
body = canonical(r)
raw = body.encode()
msg = b"k2-compute-receipt-v1\0" + struct.pack(">I", len(raw)) + raw
sig = subprocess.run(["/usr/bin/openssl", "dgst", "-sha256", "-sign", key], input=msg, stdout=subprocess.PIPE, check=True).stdout
if tamper:
    body = body.replace('"boot_id":"boot-1"', '"boot_id":"boot-2"')
    r["boot_id"] = "boot-2"
o = {"job": job[:8], "verified": True, "receipt": r, "signature": base64.b64encode(sig).decode(),
     "proves": "which node said it ran which inputs and got which outputs — not that the node is honest"}
if not no_body:
    o["body"] = body
if not no_plan:
    o["plan"] = plan
os.makedirs(os.path.dirname(out), exist_ok=True)
json.dump(o, open(out, "w"))
PY
}
# bundle <name> <files dir> [L1 node] [L1 overrides] [V1 node] [V1 overrides] [V1 files dir]:
# an audit dir like k2cr_collect writes, then compute_release.py evidence-bundle
# (the release side's builder) → $tmp/<name>/bundle.json.
bundle() {
    local name="$1" files="$2" s="$tmp/$1"
    rm -rf "$s"; mkdir -p "$s"
    mkrcpt "$s/L1/receipt.json" "${3:-build-a}" "$files" "${4:-$NOOVER}"
    [ "${5:-build-b}" = none ] || mkrcpt "$s/V1/receipt.json" "${5:-build-b}" "${7:-$files}" "${6:-$NOOVER}"
    python3 -I "$PY_CR" evidence-bundle --state "$s" --git "$G" > "$s/bundle.json" || fail "evidence-bundle $name" ""
}
# edit_bundle <name> <python on b>: change the bundle itself
edit_bundle() {
    python3 -I - "$tmp/$1/bundle.json" "$2" <<'PY'
import json, sys
p = sys.argv[1]; b = json.load(open(p))
exec(sys.argv[2])
json.dump(b, open(p, "w"))
PY
}
register() { # <name> <rc> <text> <version> <bundle name>
    check_in "$1" "$tmp/$5/bundle.json" "$2" "$3" "$K" compute-evidence "$4"
}
sign() { "$K" sign "$@" < "$SIGN_IN" > "$tmp/sig.out" 2> "$tmp/sig.err"; }
signs() { # <name> <file> <slot> <asset> <version>
    local name="$1" f="$2" slot="$3" asset="$4" v="$5" pub="$U2_LINE"
    [ "$slot" = u1 ] && pub="$U1_LINE"
    if SIGN_IN="$f" sign "$slot" "$asset" "$v" raw && "$MINISIGN" -V -P "$pub" -m "$f" -x "$tmp/sig.out" -q; then
        pass "$name"
    else
        fail "$name" "$(cat "$tmp/sig.err")"
    fi
}
refuses() { # <name> <file> <slot> <asset> <version> <text>
    local name="$1" f="$2" rc=0
    SIGN_IN="$f" sign "$3" "$4" "$5" raw || rc=$?
    if [ "$rc" = 3 ] && grep -qF -- "$6" "$tmp/sig.err" && [ ! -s "$tmp/sig.out" ]; then pass "$name"; else fail "$name" "rc=$rc $(cat "$tmp/sig.err")"; fi
}

echo "== the receipt vector: openssl-signed, verified by the signer's own code (frames.rs checks the same file) =="
python3 -I - "$TROOT/usr/local/libexec/k2sign/k2sign.py" "$REPO/tests/release/fixtures/receipt-vector.json" <<'PY' \
    && pass "the vector verifies, re-canonicalizes to its body, and a tampered body fails" || fail "receipt vector" ""
import importlib.util, json, sys, tempfile
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("k2sign_under_test", sys.argv[1])
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
v = json.load(open(sys.argv[2]))
class Ctx:
    def own(self, p): pass
der = m.spki_from_pem(v["public_key_pem"])
d = tempfile.mkdtemp()
import hashlib
assert hashlib.sha256(der).hexdigest() == v["node_fp"]
assert m.receipt_signature_ok(Ctx(), der, v["body"], v["signature"], d)
assert m.canonical_json(json.loads(v["body"])) == v["body"]
assert not m.receipt_signature_ok(Ctx(), der, v["body"].replace('"code":0', '"code":1'), v["signature"], d)
PY

echo "== sessions (Touch ID, at NOW0) =="
for v in 0.47.1 0.47.2 0.47.3 0.47.4 0.47.5 0.47.6; do
    open_session "$v" "${ASSETS[@]}"
    release_routes "$v"
    mkset "$v" "$tmp/set-$v"
done
export K2SIGN_TEST_NOW=$((NOW0 + 3600))

echo "== pins: nothing is trusted until root pins it =="
bundle good1 "$tmp/set-0.47.1"
register "no controller or nodes pinned: refused" 3 "no controller pinned" 0.47.1 good1
check "pin the controller (admin)" 0 "compute controller pinned" "$ADM" pin-compute-controller "$CTL"
register "a controller but no nodes: refused" 3 "no compute nodes pinned" 0.47.1 good1
check "M2 pin-compute-node without the fingerprint read from the node: usage" 2 "expected fingerprint" "$ADM" pin-compute-node build-a "$tmp/keys/build-a.pem"
check "M2 a key whose fingerprint is not the one read from the node: refused" 3 "not the $FP_B you read from node build-a: not pinned" \
    "$ADM" pin-compute-node build-a "$tmp/keys/build-a.pem" "$FP_B"
check "M2 a malformed expected fingerprint: usage" 2 "64 lowercase hex" "$ADM" pin-compute-node build-a "$tmp/keys/build-a.pem" "${FP_A:0:16}"
check "pin build-a with the fingerprint read from the node" 0 "$FP_A" "$ADM" pin-compute-node build-a "$tmp/keys/build-a.pem" "$FP_A"
check "pin build-b" 0 "$FP_B" "$ADM" pin-compute-node build-b "$tmp/keys/build-b.pem" "$FP_B"
check "the same key under a second name: refused" 3 "already pinned as node build-a" "$ADM" pin-compute-node build-c "$tmp/keys/build-a.pem" "$FP_A"
printf 'not a key\n' > "$tmp/keys/junk.pem"
check "a file that is not a P-256 key: refused" 3 "no PUBLIC KEY block" "$ADM" pin-compute-node build-c "$tmp/keys/junk.pem" "$FP_A"
check "pin-compute-node needs an absolute path" 2 "absolute path" "$ADM" pin-compute-node build-c keys/x.pem "$FP_A"
check "compute-pins (signer verb, read-only)" 0 "\"name\": \"build-b\"" "$K" compute-pins
grep -q "etc/compute-pins" "$TROOT/usr/local/etc/k2sign/pins.sha256" && pass "compute-pins is in pins.sha256" || fail "compute-pins pinned" "$(cat "$TROOT/usr/local/etc/k2sign/pins.sha256")"
cp "$TROOT/usr/local/etc/k2sign/compute-pins" "$tmp/pins.bak"
printf 'node build-r %s %s\n' "$FP_R" "$("$OPENSSL" ec -in "$tmp/keys/rogue.key" -pubout -outform DER 2>/dev/null | base64 | tr -d '\n')" >> "$TROOT/usr/local/etc/k2sign/compute-pins"
register "compute-pins edited without root's repin: refused" 3 "pins mismatch: compute-pins" 0.47.1 good1
cp "$tmp/pins.bak" "$TROOT/usr/local/etc/k2sign/compute-pins"

echo "== accepted: a good receipt, matching hashes and a V1 match =="
D1="$tmp/set-0.47.1/k2-daemon-linux-x86_64"
refuses "before any evidence: the x86 daemon goes the GitHub way (no attestation here)" "$D1" u2 k2-daemon-linux-x86_64 0.47.1 "no build attestation"
register "the good bundle registers" 0 '"ok": true' 0.47.1 good1
grep -q "\"built_commit\": \"$C\"" "$tmp/out" && grep -q "\"tree\": \"$TT\"" "$tmp/out" \
    && pass "the summary names the preview commit and the tag's tree" || fail "summary" "$(cat "$tmp/out")"
signs "S3b the compute-built x86 daemon signs (u2)" "$D1" u2 k2-daemon-linux-x86_64 0.47.1
signs "…and u1 (the bridge)" "$D1" u1 k2-daemon-linux-x86_64 0.47.1
grep -q $'\tCOMPUTE-EVIDENCE\t' "$AUDIT/0.47.1/signatures.tsv" && pass "the registration is in the signer's audit log" || fail "audit" ""
check "status shows the compute evidence" 0 "\"built_commit\": \"$C\"" "$K" status 0.47.1
bundle nobody "$tmp/set-0.47.1" build-a '{"no_body": true}' build-b '{"no_body": true}'
register "a receipt without its body text (an older daemon): canonical JSON, still verified" 0 '"ok": true' 0.47.1 nobody
bundle attag "$tmp/set-0.47.1" build-a "{\"src.commit\": \"$T\"}" build-b "{\"src.commit\": \"$T\"}"
edit_bundle attag 'b.pop("commit_object")'
register "built at the tag commit itself: no commit object needed" 0 '"ok": true' 0.47.1 attag
MS='["BUILD_GMAIL_CLIENT_ID", "BUILD_GMAIL_CLIENT_SECRET", "BUILD_MICROSOFT_CLIENT_ID"]'
bundle withms "$tmp/set-0.47.1" build-a "{\"secret_env\": $MS}" build-b "{\"secret_env\": $MS}"
register "the optional Microsoft client id from the vault too (both jobs)" 0 '"ok": true' 0.47.1 withms
bundle nopkgs "$tmp/set-0.47.1" build-a '{"plan.packages": null}' build-b '{"plan.packages": null}'
register "a plan without packages (a node without pkg-map installs the sys: needs itself)" 0 '"ok": true' 0.47.1 nopkgs
grep -q '"rust": "1.99.1"' "$tmp/out" && pass "the summary names the rust pin read from GitHub at the tag" || fail "rust in summary" "$(cat "$tmp/out")"

echo "== refused: the signature and the key =="
S2="$tmp/set-0.47.2"
bundle badsig "$S2" build-a '{"tamper": true}'
register "S3b-1 a bad signature (body edited after signing)" 3 "L1: the receipt's signature does not verify with node build-a's pinned key" 0.47.2 badsig
bundle rogue "$S2" rogue
register "S3b-2 the wrong key: an unpinned node" 3 "L1: the receipt is from node ${FP_R:0:16}…, which is not pinned" 0.47.2 rogue
bundle rogue2 "$S2" rogue "{\"fp\": \"$FP_A\"}"
register "S3b-2 the wrong key claiming a pinned node's fingerprint" 3 "does not verify with node build-a's pinned key" 0.47.2 rogue2
bundle rogue3 "$S2" build-a '{}' rogue
register "…and on the V1 side" 3 "V1: the receipt is from node ${FP_R:0:16}…" 0.47.2 rogue3
bundle ctl "$S2" build-a "{\"controller_fp\": \"$(printf 'ee%.0s' $(seq 32))\"}"
register "a job another controller sent" 3 "not this Mac's pinned controller" 0.47.2 ctl
bundle sigjunk "$S2"
edit_bundle sigjunk 'b["receipts"]["L1"]["signature"] = "bm90IGEgc2ln"'
register "a signature that is not DER" 3 "L1: the receipt's signature does not verify" 0.47.2 sigjunk

echo "== refused: the commit and the tree =="
bundle wrongtree "$S2" build-a "{\"src.tree\": \"$TT2\"}"
register "S3b-3 the wrong tree" 3 "L1: the job built tree ${TT2:0:12}, but v0.47.2's tree is ${TT:0:12}" 0.47.2 wrongtree
bundle wrongcommit "$S2" build-a '{}' build-b "{\"src.commit\": \"$C_BADTREE\"}"
register "S3b-3 the wrong commit (not the bundle's commit object)" 3 "V1: the job built commit ${C_BADTREE:0:12}, not the bundle's commit ${C:0:12}" 0.47.2 wrongcommit
bundle badparent "$S2" build-a "{\"src.commit\": \"$C_BADPARENT\"}" build-b "{\"src.commit\": \"$C_BADPARENT\"}"
register "S3b-3 a commit on another parent than the approved base" 3 "parent is ${T:0:12}, not the approved base ${B:0:12}" 0.47.2 badparent
bundle badctree "$S2" build-a "{\"src.commit\": \"$C_BADTREE\", \"src.tree\": \"$TT\"}" build-b "{\"src.commit\": \"$C_BADTREE\", \"src.tree\": \"$TT\"}"
register "S3b-3 a commit object whose tree is not the tag's" 3 "has tree ${TT2:0:12}, not v0.47.2's ${TT:0:12}" 0.47.2 badctree
bundle noobj "$S2"
edit_bundle noobj 'b.pop("commit_object")'
register "a preview commit with no commit object to bind it" 3 "the bundle has no commit object to bind it" 0.47.2 noobj
bundle fakeobj "$S2"
edit_bundle fakeobj 'import base64; o = base64.b64decode(b["commit_object"]); b["commit_object"] = base64.b64encode(o.replace(b"release preview", b"release previeX")).decode()'
register "an edited commit object (its hash no longer matches)" 3 "not the bundle's commit" 0.47.2 fakeobj
bundle dirty "$S2" build-a "{\"src.dirty_sha256\": \"$(printf 'd%.0s' $(seq 64))\"}"
register "uncommitted changes (--dirty)" 3 "ran uncommitted changes" 0.47.2 dirty

echo "== refused: what ran =="
bundle argv "$S2" build-a "{\"argv_sha256\": \"$(printf '0%.0s' $(seq 64))\"}"
register "another command than the release build" 3 "did not run the release build command (bash scripts/ci/build-linux-bins.sh x86_64)" 0.47.2 argv
bundle failed "$S2" build-a '{"exit": {"code": 1, "signal": null}}'
register "a job that exited 1" 3 "did not end done/exit 0" 0.47.2 failed
bundle cache "$S2" build-a '{"env_names": ["BUILD_COMMIT", "HOME", "K2_JOB_CACHE"]}'
register "a job with a --cache mount" 3 "ran with a --cache mount" 0.47.2 cache
bundle env "$S2" build-a '{"env_names": ["BUILD_COMMIT", "HOME", "RUSTFLAGS"]}'
register "a job with extra --env" 3 "env the release build doesn't set: RUSTFLAGS" 0.47.2 env
bundle placeholder "$S2" build-a '{"env_names": ["BUILD_COMMIT", "BUILD_OAUTH", "HOME"]}'
register "a placeholder OAuth build (BUILD_OAUTH)" 3 "env the release build doesn't set: BUILD_OAUTH" 0.47.2 placeholder

echo "== refused: the plan (S3b review H1) =="
bundle noplan "$S2" build-a '{"no_plan": true}'
register "H1 no plan in the evidence (an older daemon's receipt)" 3 "L1: the evidence has no job plan" 0.47.2 noplan
bundle plantamper "$S2" build-a '{"plan_tamper": {"cwd": "crates"}}'
register "H1 a plan that doesn't hash to the receipt's plan_digest" 3 "L1: the plan does not hash to the receipt's plan_digest" 0.47.2 plantamper
NEST='[{"path": "crates/k2-daemon", "project_key": "pk-evil", "remote_url": null, "commit": "'"$C_BADTREE"'"}]'
bundle nested "$S2" build-a "{\"plan.with\": $NEST, \"plan.src_path\": \"\"}" build-b "{\"plan.with\": $NEST, \"plan.src_path\": \"\"}"
register "H1 exploit: a --with repo nested at crates/k2-daemon, right argv and tree, both pinned nodes" 3 \
    "L1: the plan ships --with repos (crates/k2-daemon): they would lay other code into the build" 0.47.2 nested
bundle srcpath "$S2" build-a '{"plan.src_path": "k2"}'
register "H1 a --with layout (src_path) without repos" 3 "the plan has a --with layout (src_path 'k2')" 0.47.2 srcpath
bundle cwd "$S2" build-a '{"plan.cwd": "crates"}'
register "H1 a cwd" 3 "the plan runs in another folder (cwd 'crates')" 0.47.2 cwd
bundle plancache "$S2" build-a '{"plan.cache": "target"}'
register "H1 a --cache in the plan" 3 "the plan mounts a --cache ('target')" 0.47.2 plancache
bundle pkgs "$S2" build-a '{"plan.packages": ["evil-tools", "pkg-config", "zlib1g-dev"]}'
register "H1 packages beyond the release needs" 3 "installs packages beyond the release needs: evil-tools" 0.47.2 pkgs
bundle needstable "$S2" build-a '{"plan.needs": ["rust@stable", "cmake@3", "sys:pkg-config", "sys:zlib1g-dev"]}'
register "H1 needs: rust@stable, not the pin" 3 "the plan needs rust@stable cmake@3" 0.47.2 needstable
bundle needextra "$S2" build-a '{"plan.needs": ["rust@1.99.1", "cmake@3", "sys:pkg-config", "sys:zlib1g-dev", "node@22"]}'
register "H1 needs: an extra tool" 3 "the release build needs rust@1.99.1 cmake@3 sys:pkg-config sys:zlib1g-dev" 0.47.2 needextra
bundle planenv "$S2" build-a "{\"plan.env\": {\"BUILD_COMMIT\": \"$C\", \"BUILD_TREE\": \"$TT\", \"BUILD_GMAIL_CLIENT_ID\": \"x\"}}"
register "H1 a free-form BUILD_GMAIL_* value in the plan's env" 3 "the plan sets env the release build doesn't: BUILD_GMAIL_CLIENT_ID" 0.47.2 planenv
bundle envval "$S2" build-a "{\"plan.env\": {\"BUILD_COMMIT\": \"$T\", \"BUILD_TREE\": \"$TT\"}}"
register "H1 BUILD_COMMIT is not the commit it ran" 3 "BUILD_COMMIT/BUILD_TREE are not the commit and tree it ran" 0.47.2 envval
bundle planarts "$S2" build-a '{"plan.artifacts": ["dist/*", "../*"]}'
register "H1 other artifact globs" 3 "the plan collects" 0.47.2 planarts
bundle planfield "$S2" build-a '{"plan.future_switch": 1}'
register "H1 a plan field the signer doesn't know" 3 "the plan has fields the signer doesn't know: future_switch" 0.47.2 planfield
bundle planargv "$S2" build-a '{"plan.argv": ["bash", "-c", "x"]}'
register "H1 the plan's command (argv) is not the release build" 3 "the plan's command is not the release build command" 0.47.2 planargv
bundle rustc "$S2" build-a '{"tools_used": {"rustc": "1.98.0"}}'
register "H1 the rustc that ran is not the tag's pin" 3 "L1: the job's rustc was 1.98.0, the tag pins 1.99.1" 0.47.2 rustc

echo "== refused: shared caches and the OAuth path (S3b review H2) =="
bundle warm "$S2" build-a '{"src.warmth": "warm", "src.slot": 1}'
register "H2 a build in a node's warm target slot" 3 "L1: the job built in a shared target dir (warmth 'warm')" 0.47.2 warm
bundle nowarmth "$S2" build-a '{"src.warmth": null}'
register "H2 no warmth said" 3 "built in a shared target dir (warmth None)" 0.47.2 nowarmth
bundle envmode "$S2" build-a '{"secret_env": [], "env_names": ["BUILD_COMMIT", "BUILD_GMAIL_CLIENT_ID", "BUILD_GMAIL_CLIENT_SECRET", "BUILD_TREE", "HOME"]}'
register "H2 an env-mode job (the OAuth values in --env)" 3 "env the release build doesn't set: BUILD_GMAIL_CLIENT_ID, BUILD_GMAIL_CLIENT_SECRET" 0.47.2 envmode
bundle nosecret "$S2" build-a '{"secret_env": []}'
register "H2 no vault secrets at all (not a secret job)" 3 "did not get the OAuth values from the vault (secret_env lacks BUILD_GMAIL_CLIENT_ID, BUILD_GMAIL_CLIENT_SECRET)" 0.47.2 nosecret
bundle onesecret "$S2" build-a '{"secret_env": ["BUILD_GMAIL_CLIENT_ID"]}'
register "H2 only part of the OAuth secrets" 3 "secret_env lacks BUILD_GMAIL_CLIENT_SECRET" 0.47.2 onesecret
bundle extrasecret "$S2" build-a '{"secret_env": ["BUILD_GMAIL_CLIENT_ID", "BUILD_GMAIL_CLIENT_SECRET", "BUILD_SIGNING_KEY"]}'
register "H2 a vault secret the build doesn't use" 3 "vault secrets the release build doesn't use: BUILD_SIGNING_KEY" 0.47.2 extrasecret
bundle secretmismatch "$S2" build-a '{"plan.secret_env": ["BUILD_GMAIL_CLIENT_ID", "BUILD_GMAIL_CLIENT_SECRET", "BUILD_MICROSOFT_CLIENT_ID"]}'
register "H2 the plan's secrets are not the receipt's" 3 "the plan's vault secrets are not the receipt's" 0.47.2 secretmismatch
bundle v1ms "$S2" build-a '{}' build-b "{\"secret_env\": $MS}"
register "V1 got other vault secrets than L1" 3 "V1 got other vault secrets than L1" 0.47.2 v1ms

echo "== refused: the V1 dual build =="
bundle nov1 "$S2" build-a '{}' none
register "S3b-5 no V1 receipt at all" 3 "no V1 receipt: the dual build on a second node is required" 0.47.2 nov1
bundle v1miss "$S2" build-a '{}' build-b '{"drop_art": ["k2-node-linux-x86_64"]}'
register "S3b-5 V1 did not build k2-node (missing V1 match)" 3 "V1 match missing: V1's receipt lists no k2-node-linux-x86_64" 0.47.2 v1miss
bundle v1diff "$S2" build-a '{}' build-b "{\"set_art\": {\"k2-root-helper-linux-x86_64\": \"$(printf 'f%.0s' $(seq 64))\"}}"
register "S3b-6 V1 mismatch on the root helper" 3 "V1 mismatch: k2-root-helper-linux-x86_64 differs between L1 on build-a" 0.47.2 v1diff
mkset 0.47.2 "$tmp/set-0.47.2-v1"
cp "$S2"/k2-root-helper-linux-x86_64 "$S2"/k2-node-linux-x86_64 "$tmp/set-0.47.2-v1/"
bundle v1engine "$S2" build-a '{}' build-b '{}' "$tmp/set-0.47.2-v1"
register "S3b-6 V1 mismatch on the App engine (other bytes)" 3 "V1 mismatch: k2-app-engine-linux-x86_64 differs" 0.47.2 v1engine
bundle samenode "$S2" build-a '{}' build-a
register "V1 on the same node as L1" 3 "V1 ran on the same node as L1 (build-a)" 0.47.2 samenode

echo "== refused: old and replayed receipts =="
bundle old "$S2" build-a "{\"started_at\": $((NOW0 - 3600)), \"ended_at\": $((NOW0 - 1800))}"
register "S3b-7 an old receipt: the job started before this version's approval" 3 "L1: an old receipt: the job started" 0.47.2 old
bundle future "$S2" build-a "{\"ended_at\": $((NOW0 + 3600 + 4000))}"
register "a receipt that ends in the future" 3 "ends in the future" 0.47.2 future
# 0.47.3's session is as old as 0.47.1's, so only the registry can catch this.
cp "$tmp/good1/bundle.json" "$tmp/replay.json"; mkdir -p "$tmp/replay"; cp "$tmp/replay.json" "$tmp/replay/bundle.json"
register "S3b-7 a replayed receipt: 0.47.1's jobs offered for 0.47.3" 3 "a replayed receipt: it was registered for 0.47.1" 0.47.3 replay
grep -q $'\tREFUSED-COMPUTE-EVIDENCE\t' "$AUDIT/0.47.3/signatures.tsv" && pass "evidence refusals are in the signer's audit log" || fail "refusal audit" ""

echo "== refused at signing: the file's hash and the receipt's artifact list =="
S4="$tmp/set-0.47.4"
bundle good4 "$S4"
register "0.47.4's good bundle registers" 0 '"ok": true' 0.47.4 good4
mkset 0.47.4 "$tmp/other-0.47.4"
refuses "S3b-4 a hash mismatch: other bytes than L1's signed artifact" "$tmp/other-0.47.4/k2-daemon-linux-x86_64" u2 k2-daemon-linux-x86_64 0.47.4 "are not L1's signed artifact"
IDENT4="https://github.com/Alakazam-211/K2/.github/workflows/daemon-binaries.yml@refs/tags/v0.47.4"
python3 -I - "$FIX/attest" "$tmp/other-0.47.4/k2-daemon-linux-x86_64" "$IDENT4" "refs/tags/v0.47.4" "$T" <<'PY'
import hashlib, json, os, sys
d, f, ident, ref, commit = sys.argv[1:6]
sha = hashlib.sha256(open(f, "rb").read()).hexdigest()
json.dump([{"verificationResult": {"statement": {"subject": [{"name": os.path.basename(f), "digest": {"sha256": sha}}]},
            "signature": {"certificate": {"subjectAlternativeName": ident, "sourceRepositoryRef": ref, "sourceRepositoryDigest": commit}}}}],
          open(os.path.join(d, sha + ".json"), "w"))
PY
refuses "registered compute evidence decides: a GitHub attestation for other bytes doesn't get around it" "$tmp/other-0.47.4/k2-daemon-linux-x86_64" u2 k2-daemon-linux-x86_64 0.47.4 "are not L1's signed artifact"
signs "0.47.4's own compute-built daemon signs" "$S4/k2-daemon-linux-x86_64" u2 k2-daemon-linux-x86_64 0.47.4
S5="$tmp/set-0.47.5"
bundle noart "$S5" build-a '{"drop_art": ["k2-daemon-linux-x86_64"]}'
register "a bundle whose L1 lists no daemon still registers (the V1 set matches)" 0 '"ok": true' 0.47.5 noart
refuses "S3b-8 an artifact not listed in L1's receipt" "$S5/k2-daemon-linux-x86_64" u2 k2-daemon-linux-x86_64 0.47.5 "k2-daemon-linux-x86_64 is not an artifact in L1's signed receipt"
refuses "aarch64 is never a compute asset: GitHub's attestation is still required" "$S5/k2-daemon-linux-aarch64" u2 k2-daemon-linux-aarch64 0.47.5 "no build attestation"

echo "== L1: clear-compute-evidence (root) puts a version back on GitHub's path =="
check "L1 clear 0.47.5's compute evidence (admin, audited)" 0 "compute evidence for 0.47.5 cleared" "$ADM" clear-compute-evidence 0.47.5
grep -q $'\tCOMPUTE-EVIDENCE-CLEARED-BY-ADMIN\t' "$AUDIT/0.47.5/signatures.tsv" && pass "the clear is in the signer's audit log" || fail "clear audit" ""
refuses "after the clear, the x86 daemon goes the GitHub way (attestation needed)" "$S5/k2-daemon-linux-x86_64" u2 k2-daemon-linux-x86_64 0.47.5 "no build attestation"
check "clearing again: nothing to clear" 3 "0.47.5 has no compute evidence registered" "$ADM" clear-compute-evidence 0.47.5
check "the release user can't clear (no signer verb)" 2 "verbs:" "$K" clear-compute-evidence 0.47.5
check "status after the clear: no compute evidence" 0 '"compute": null' "$K" status 0.47.5

echo "== the test switch for per-file evidence never reaches compute-evidence =="
K2SIGN_TEST_EVIDENCE=accept register "K2SIGN_TEST_EVIDENCE=accept: a bad bundle is still refused" 3 "does not verify" 0.47.2 badsig

echo "== the rust pin comes from GitHub at the tag =="
route "$REPOP/contents/rust-toolchain.toml?ref=$T" -
register "no rust-toolchain.toml at the tag: refused" 3 "v0.47.2 has no rust-toolchain.toml on GitHub" 0.47.2 good4
TWO_B64="$(printf '[toolchain]\nchannel = "1.99.1"\nchannel = "1.98.0"\n' | base64 | tr -d '\n')"
route "$REPOP/contents/rust-toolchain.toml?ref=$T" "{\"encoding\": \"base64\", \"content\": \"$TWO_B64\"}"
register "a rust-toolchain.toml with two channels: refused" 3 "pins 2 channels, want exactly 1" 0.47.2 good4
OLD_B64="$(printf '[toolchain]\nchannel = "1.98.0"\n' | base64 | tr -d '\n')"
route "$REPOP/contents/rust-toolchain.toml?ref=$T" "{\"encoding\": \"base64\", \"content\": \"$OLD_B64\"}"
register "the tag pins another Rust than the jobs built with: refused" 3 "the job's rustc was 1.99.1, the tag pins 1.98.0" 0.47.2 good4
release_routes 0.47.2

echo "== no session, no evidence =="
bundle nosess "$tmp/set-0.47.1"
register "a version with no approved session" 3 "no approved signing session for 0.47.9" 0.47.9 nosess

echo "== release side: sign-daemon.sh --dry under K2_SIGNER=k2sign (verdict dir → bundle → signer) =="
export K2_UPDATER_SRC_ROOT="$TREE" K2_RELEASE_AUDIT_DIR="$tmp/home-audit" HOME="$tmp/home" K2_SIGNER=k2sign K2_SIGN_SOURCE_DIR="$G"
mkdir -p "$HOME"
V6=0.47.6 S6="$tmp/set-0.47.6"
cat > "$tmp/WHATS_NEW.md" <<MD
## $V6 — Test
- a line
MD
ARTS='    "macos-aarch64": {
      "url": "https://github.com/Alakazam-211/K2/releases/download/v0.47.6/k2-daemon-macos-aarch64",
      "sig": "https://github.com/Alakazam-211/K2/releases/download/v0.47.6/k2-daemon-macos-aarch64.sig",
      "sha256": "abc"
    }'
bash -c "source '$REPO/scripts/daemon-manifest.sh'; k2_daemon_manifest $V6 2026-11-06T00:00:00Z '$ARTS' '$tmp/WHATS_NEW.md'" > "$tmp/daemon-latest.json"
# The audit dir release.sh keeps: L1/V1 receipts (+ the verdict the early check reads).
bundle audit6 "$S6"
python3 -I - "$S6" "$tmp/audit6" <<'PY'
import hashlib, json, os, sys
d, out = sys.argv[1:3]
files = {"dist/" + n: {"sha256": hashlib.sha256(open(os.path.join(d, n), "rb").read()).hexdigest(), "bytes": 1}
         for n in sorted(os.listdir(d)) if n.startswith("k2-daemon-linux-") and not n.endswith(".sha256")}
json.dump({"label": "L1", "ok": True, "files": files, "problems": []}, open(os.path.join(out, "L1.verdict.json"), "w"))
PY
python3 -I - "$FIX/attest" "$S6/k2-daemon-linux-aarch64" "https://github.com/Alakazam-211/K2/.github/workflows/daemon-binaries.yml@refs/tags/v$V6" "refs/tags/v$V6" "$T" <<'PY'
import hashlib, json, os, sys
d, f, ident, ref, commit = sys.argv[1:6]
sha = hashlib.sha256(open(f, "rb").read()).hexdigest()
json.dump([{"verificationResult": {"statement": {"subject": [{"name": os.path.basename(f), "digest": {"sha256": sha}}]},
            "signature": {"certificate": {"subjectAlternativeName": ident, "sourceRepositoryRef": ref, "sourceRepositoryDigest": commit}}}}],
          open(os.path.join(d, sha + ".json"), "w"))
PY
# A bad verdict dir first: V1's receipt edited (a mismatch): nothing is signed.
cp -R "$tmp/audit6" "$tmp/audit6-bad"
mkrcpt "$tmp/audit6-bad/V1/receipt.json" build-b "$S6" "{\"set_art\": {\"k2-node-linux-x86_64\": \"$(printf 'a%.0s' $(seq 64))\"}}"
rc=0
bash "$REPO/scripts/sign-daemon.sh" "$V6" --dry --from-dir "$S6" --compute-verdict "$tmp/audit6-bad" \
    --manifest "$tmp/daemon-latest.json" --out "$tmp/sd-bad" > "$tmp/sd-bad.log" 2>&1 || rc=$?
if [ "$rc" = 1 ] && grep -q "V1 mismatch: k2-node-linux-x86_64" "$tmp/sd-bad.log" && grep -q "refused the K2 Compute evidence" "$tmp/sd-bad.log" \
    && ! ls "$tmp/sd-bad"/*.sig >/dev/null 2>&1; then
    pass "sign-daemon.sh stops at a bad V1 receipt: the signer refuses, nothing is signed"
else
    fail "sign-daemon bad verdict" "rc=$rc $(tail -5 "$tmp/sd-bad.log")"
fi
rc=0
bash "$REPO/scripts/sign-daemon.sh" "$V6" --dry --from-dir "$S6" --compute-verdict "$tmp/audit6" \
    --manifest "$tmp/daemon-latest.json" --out "$tmp/sd" > "$tmp/sd.log" 2>&1 || rc=$?
if [ "$rc" = 0 ]; then pass "sign-daemon.sh --dry signs the compute-built x86 daemon through the signer (and aarch64 the GitHub way)"; else fail "sign-daemon dry" "rc=$rc $(tail -15 "$tmp/sd.log")"; fi
grep -q "registered the K2 Compute evidence for $V6" "$tmp/sd.log" && pass "…after the signer registered the evidence itself" || fail "registration line" "$(cat "$tmp/sd.log")"
[ -s "$tmp/audit6/k2sign-compute-evidence.json" ] && pass "the signer's answer is kept in the audit dir" || fail "audit copy" ""
bash -c 'source "$1"; k2u_verify_u2 "$2" "$3" k2-daemon-linux-x86_64 "$4"' _ "$TREE/scripts/lib/sign-updater.sh" "$S6/k2-daemon-linux-x86_64" "$tmp/sd/k2-daemon-linux-x86_64.sig2" "$V6" \
    && pass "the x86 .sig2 verifies the way 0.46.6+ installs check" || fail "x86 sig2" ""
grep -q "\"sha256\": \"$(shasum -a 256 "$S6/k2-daemon-linux-x86_64" | awk '{print $1}')\"" "$tmp/sd/daemon-latest.json" \
    && pass "daemon-latest.json lists the compute-built x86 daemon" || fail "manifest" "$(cat "$tmp/sd/daemon-latest.json")"
unset K2_SIGNER K2_SIGN_SOURCE_DIR

echo "== policy drift =="
python3 -I - "$REPO/scripts/lib/compute-release.sh" "$REPO/scripts/k2sign/policy" <<'PY' && pass "policy compute_dual_names == k2cr_dual_names; compute_argv == the L1/V1 command" || fail "compute policy drift"
import re, sys
s, pol = open(sys.argv[1]).read(), {}
for line in open(sys.argv[2]).read().splitlines():
    if "=" in line and not line.startswith("#"):
        k, v = line.split("=", 1)
        pol[k.strip()] = v.strip()
dual = re.search(r'k2cr_dual_names\(\) \{\n\s*printf .%s. "([^"]+)"', s).group(1).split(",")
assert dual == pol["compute_dual_names"].split(), (dual, pol["compute_dual_names"])
for label in ("L1", "V1"):
    m = re.search(r'k2cr_submit %s .*? -- (bash scripts/ci/build-linux-bins\.sh x86_64)' % label, s)
    assert m and m.group(1) == pol["compute_argv"], label
needs = re.search(r'local bins_needs="\$rust,([^"]+)"', s).group(1).split(",")
assert ["rust@{rust_pin}"] + needs == pol["compute_needs"].split(), (needs, pol["compute_needs"])
assert re.search(r"--artifact '([^']+)'", s).group(1) == pol["compute_artifacts"]
PY
python3 -I - "$REPO/crates/k2-node-proto/src/frames.rs" "$TROOT/usr/local/libexec/k2sign/k2sign.py" <<'PY' && pass "the signer's PLAN_KEYS are exactly frames.rs Plan's fields" || fail "plan field drift"
import importlib.util, re, sys
sys.dont_write_bytecode = True
src = open(sys.argv[1]).read()
body = re.search(r"pub struct Plan \{\n(.*?)\n\}", src, re.S).group(1)
fields = set(re.findall(r"^    pub (\w+):", body, re.M))
spec = importlib.util.spec_from_file_location("k2sign_drift", sys.argv[2])
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
assert fields == m.PLAN_KEYS, (sorted(fields ^ m.PLAN_KEYS))
PY

finish
