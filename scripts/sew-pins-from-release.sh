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
#     in a `<tarball>.sha256` file, must match release.json.
# Any mismatch exits 1 and prints nothing on stdout. The `media`,
# `transcriber` and `recorder` sidecars are pinned; any other is refused
# (K2's table has no slot for it).
#
# Default output is a `SewRelease { … }` expression for SEW_RELEASES;
# `--const NAME` wraps it as `const NAME: SewRelease = …;` (the test tables).
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
PY
