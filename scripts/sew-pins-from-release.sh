#!/usr/bin/env bash
# sew-pins-from-release.sh — print one Sew pin-table entry for
# crates/k2-root-proto/src/sew.rs from a Sew release's release.json (the
# contract: version, per-arch tarball sha256 + binary_sha256, sidecars with
# their own version and artifacts). K2-PIN.txt is hand-made and not
# authoritative (Sew, 2026-10-16).
#
#   bash scripts/sew-pins-from-release.sh <release-dir> [--const NAME]
#
# <release-dir> holds sew-<v>-release.json and SHA256SUMS (and, when
# present, the tarballs themselves). Before printing anything it checks:
#   - every core artifact's tarball and binary sha256 against SHA256SUMS
#     (both must be listed: `<sha>  <tarball>` and the binary as either
#     `<sha>  sew (extracted from <tarball>)` (0.1.x) or
#     `<sha>  <tarball without .tar.gz>/sew` (0.2.0 on));
#   - every tarball that is in the directory: its sha256, and the sha256 of
#     its single binary member (`sew` or `sew-<sidecar>`), against
#     release.json; licence members beside it (`LICENSE`, `NOTICE`,
#     `THIRD_PARTY_NOTICES.md`, from 0.2.0) must be regular files;
#   - sidecar tarballs and binaries listed in SHA256SUMS (either form), or
#     in a `<tarball>.sha256` file, must match release.json;
#   - Sew 0.5.0 on, the App kit (`app_kit: {file, sha256}`,
#     sew-app-kit-<v>.tar.gz): listed in SHA256SUMS with the same sha256;
#     when the tarball is here, its bytes hash to it and it holds exactly
#     one regular kit.json (of this version, if kit.json names one).
# Any mismatch exits 1 and prints nothing on stdout. The `media`,
# `transcriber` and `recorder` sidecars are pinned; any other is refused
# (K2's table has no slot for it). A pre-release (Sew's canary) pins like
# any release: K2 records the tag and the hashes exactly as published.
#
# Before the entry, a `// provenance:` line names the tag (v<version>),
# whether release.json marks it a pre-release, its commit when release.json
# carries one, and the sha256 of SHA256SUMS and of release.json, for the
# pin's doc comment (as 0.3.1's names them).
#
# Default output is a `SewRelease { … }` expression for SEW_RELEASES;
# `--const NAME` wraps it as `const NAME: SewRelease = …;` (the test tables).
# With an `app_kit`, a `SewAppKit { … },` row for SEW_APP_KITS follows.
set -euo pipefail

usage() { echo "usage: $0 <release-dir> [--const NAME]" >&2; exit 2; }
[ $# -ge 1 ] || usage
DIR="$1"; shift
CONST=""
while [ $# -gt 0 ]; do
	case "$1" in
		--const) CONST="${2:-}"; [ -n "$CONST" ] || usage; shift 2 ;;
		*) usage ;;
	esac
done
[ -d "$DIR" ] || { echo "no such directory: $DIR" >&2; exit 2; }

exec python3 -I - "$DIR" "$CONST" <<'PY'
import glob, hashlib, io, json, os, re, sys, tarfile

d, const = sys.argv[1], sys.argv[2]
rel_files = glob.glob(os.path.join(d, "sew-*-release.json"))
if len(rel_files) != 1:
    sys.exit(f"expected exactly one sew-<v>-release.json in {d}, found {len(rel_files)}")
rel = json.load(open(rel_files[0]))
sums_path = os.path.join(d, "SHA256SUMS")
if not os.path.isfile(sums_path):
    sys.exit(f"no SHA256SUMS in {d}")
sums = {}
for line in open(sums_path):
    line = line.rstrip("\n")
    if not line or line.startswith("#"):
        continue
    m = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
    if not m:
        sys.exit(f"SHA256SUMS: unreadable line: {line!r}")
    sums[m.group(2)] = m.group(1)

errors = []
HEX = re.compile(r"[0-9a-f]{64}")

def sha(b):
    return hashlib.sha256(b).hexdigest()

NOTICES = ("LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.md")

def check_file(art, member):
    """Hash the tarball and its binary member when the tarball is here."""
    p = os.path.join(d, art["file"])
    if not os.path.isfile(p):
        return False
    data = open(p, "rb").read()
    if sha(data) != art["sha256"]:
        errors.append(f"{art['file']}: sha256 {sha(data)} != release.json {art['sha256']}")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as t:
        ms = [m for m in t.getmembers() if m.name.lstrip("./") == member and m.isfile()]
        if len(ms) != 1:
            errors.append(f"{art['file']}: no single regular member '{member}'")
        else:
            b = t.extractfile(ms[0]).read()
            if sha(b) != art["binary_sha256"]:
                errors.append(f"{art['file']}: {member} sha256 {sha(b)} != release.json {art['binary_sha256']}")
        for m in t.getmembers():
            name = m.name.lstrip("./")
            if name in NOTICES and not m.isfile():
                errors.append(f"{art['file']}: licence member '{name}' is not a regular file")
    return True

def binary_sum(tarball, member):
    """The binary's line in SHA256SUMS, in either form."""
    stem = tarball[: -len(".tar.gz")] if tarball.endswith(".tar.gz") else tarball
    for key in (f"{member} (extracted from {tarball})", f"{stem}/{member}"):
        if key in sums:
            return key, sums[key]
    return f"{stem}/{member}", None

def shape(art, where):
    for k in ("arch", "triple", "file", "sha256", "binary_sha256"):
        if k not in art:
            errors.append(f"{where}: artifact without {k}")
            return False
    for k in ("sha256", "binary_sha256"):
        if not HEX.fullmatch(art[k]):
            errors.append(f"{where} {art['arch']}: {k} is not 64 lowercase hex")
            return False
    return True

version = rel.get("version")
if not version or not re.fullmatch(r"\d+\.\d+\.\d+", version):
    sys.exit(f"release.json: bad version {version!r}")
core = rel.get("artifacts") or []
if not core:
    errors.append("release.json: no artifacts")
for a in core:
    if not shape(a, "artifacts"):
        continue
    if sums.get(a["file"]) != a["sha256"]:
        errors.append(f"{a['file']}: SHA256SUMS has {sums.get(a['file'])}, release.json {a['sha256']}")
    key, listed = binary_sum(a["file"], "sew")
    if listed != a["binary_sha256"]:
        errors.append(f"{key}: SHA256SUMS has {listed}, release.json {a['binary_sha256']}")
    check_file(a, "sew")

SIDECARS = ("media", "transcriber", "recorder")
side_rel = {}
for s in rel.get("sidecars") or []:
    name = s.get("name")
    if name not in SIDECARS:
        errors.append(f"sidecar {name!r}: K2's pin table has no slot for it")
        continue
    if name in side_rel:
        errors.append(f"sidecar {name}: listed twice")
        continue
    if not s.get("version") or not re.fullmatch(r"\d+\.\d+\.\d+", s["version"]):
        errors.append(f"sidecar {name}: bad version {s.get('version')!r}")
    member = f"sew-{name}"
    for a in s.get("artifacts") or []:
        if not shape(a, f"sidecar {name}"):
            continue
        if a["file"] in sums and sums[a["file"]] != a["sha256"]:
            errors.append(f"{a['file']}: SHA256SUMS has {sums[a['file']]}, release.json {a['sha256']}")
        key, listed = binary_sum(a["file"], member)
        if listed is not None and listed != a["binary_sha256"]:
            errors.append(f"{key}: SHA256SUMS has {listed}, release.json {a['binary_sha256']}")
        side = os.path.join(d, a["file"] + ".sha256")
        if os.path.isfile(side):
            listed = open(side).read().split()[0]
            if listed != a["sha256"]:
                errors.append(f"{a['file']}.sha256 has {listed}, release.json {a['sha256']}")
        check_file(a, member)
    side_rel[name] = s

# Sew 0.5.0 on: the App kit (`app_kit`: sew-app-kit-<v>.tar.gz). Its sha256
# must be in SHA256SUMS and equal release.json's; when the tarball is here,
# its bytes are hashed too and it must hold exactly one regular kit.json
# (whose version, if it names one, is the release's).
kit = rel.get("app_kit")
if kit is not None:
    if not isinstance(kit, dict) or not isinstance(kit.get("file"), str) or not isinstance(kit.get("sha256"), str):
        errors.append("app_kit: expected {file, sha256}")
        kit = None
    elif not HEX.fullmatch(kit["sha256"]):
        errors.append("app_kit: sha256 is not 64 lowercase hex")
        kit = None
    else:
        if kit.get("version") not in (None, version):
            errors.append(f"app_kit: version {kit.get('version')!r} is not the release's {version}")
        if kit["file"] != f"sew-app-kit-{version}.tar.gz":
            errors.append(f"app_kit: file {kit['file']!r} is not sew-app-kit-{version}.tar.gz")
        if sums.get(kit["file"]) != kit["sha256"]:
            errors.append(f"{kit['file']}: SHA256SUMS has {sums.get(kit['file'])}, release.json {kit['sha256']}")
        p = os.path.join(d, kit["file"])
        if os.path.isfile(p):
            data = open(p, "rb").read()
            if sha(data) != kit["sha256"]:
                errors.append(f"{kit['file']}: sha256 {sha(data)} != release.json {kit['sha256']}")
            with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as t:
                kj = [m for m in t.getmembers() if os.path.basename(m.name.rstrip("/")) == "kit.json"]
                if len(kj) != 1 or not kj[0].isfile():
                    errors.append(f"{kit['file']}: no single regular kit.json")
                else:
                    try:
                        kv = json.load(t.extractfile(kj[0])).get("version")
                    except Exception as e:
                        errors.append(f"{kit['file']}: kit.json unreadable ({e})")
                        kv = None
                    if kv not in (None, version):
                        errors.append(f"{kit['file']}: kit.json version {kv!r} is not {version}")
        else:
            print(f"sew-pins-from-release: note: {kit['file']} is not in {d}; its sha256 is checked against SHA256SUMS only", file=sys.stderr)

# A pre-release (Sew's canary) pins like any release (Rosson 2026-10-28:
# K2 0.46.6 ships with Sew 0.5.0 once Sew's cut passes its smoke run).
prerelease = rel.get("prerelease") is True or rel.get("channel") not in (None, "stable")

if errors:
    for e in errors:
        print("sew-pins-from-release: " + e, file=sys.stderr)
    sys.exit(1)

def arts(lst, ind):
    out = []
    for a in sorted(lst, key=lambda a: a["arch"]):
        out.append(
            f"{ind}SewArtifact {{\n"
            f"{ind}    arch: \"{a['arch']}\",\n"
            f"{ind}    triple: \"{a['triple']}\",\n"
            f"{ind}    tarball_sha256: \"{a['sha256']}\",\n"
            f"{ind}    binary_sha256: \"{a['binary_sha256']}\",\n"
            f"{ind}}}"
        )
    return out

def artslice(lst, ind):
    a = arts(lst, ind + "    ")
    if len(a) == 1:
        # One artifact: `&[SewArtifact { … }]` on the field's own lines.
        body = a[0].strip()
        return "&[" + body.replace("\n" + ind + "    ", "\n" + ind) + "]"
    return "&[\n" + ",\n".join(a) + ",\n" + ind + "]"

commit = rel.get("git_sha") or rel.get("commit")
prov = [f"sew-releases tag v{version}"]
if prerelease:
    prov.append("published as a pre-release")
if isinstance(commit, str) and commit:
    prov.append(f"commit {commit}")
prov.append(f"SHA256SUMS sha256 {sha(open(sums_path, 'rb').read())}")
prov.append(f"{os.path.basename(rel_files[0])} sha256 {sha(open(rel_files[0], 'rb').read())}")
print("// provenance: " + ", ".join(prov))

ind = ""
lines = ["SewRelease {", f"    version: \"{version}\",", f"    artifacts: {artslice(core, '    ')},"]
for name in SIDECARS:
    s = side_rel.get(name)
    if s:
        lines += [
            f"    {name}: Some(SewSidecar {{",
            f"        version: \"{s['version']}\",",
            f"        artifacts: {artslice(s['artifacts'], '        ')},",
            "    }),",
        ]
    else:
        lines.append(f"    {name}: None,")
lines.append("}")
expr = "\n".join(lines)
if const:
    print(f"const {const}: SewRelease = {expr};")
else:
    print(expr + ",")
if kit is not None:
    # The SEW_APP_KITS row (same change as the release entry).
    print(f"// SEW_APP_KITS:\nSewAppKit {{ version: \"{version}\", sha256: \"{kit['sha256']}\" }},")
PY
