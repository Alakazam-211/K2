#!/usr/bin/env python3
"""compute_release.py — the checks release.sh runs on Linux builds that
came back from K2 Compute (plan-ci-release-on-k2-compute-v1 §2 step 5,
§6). Pure file work: it reads what `k2 compute get/receipt/fetch --json`
printed and the fetched files; it never talks to a daemon or GitHub,
never runs a fetched binary.

  compute_release.py argv-sha256 -- ARG...
      The receipt's argv_sha256 for ARGs (canonical JSON array).

  compute_release.py check-job --label L --get get.json --receipt r.json
      --fetched fetch.json --commit C --tree T --argv-json '[...]'
      --nodes 'k2n4-vin,k2n5-vin' --version V --kind linux-bins|linux-app
      [--arch x86_64] [--require-oauth baked]
      One job: ended done/exit 0, receipt verified by the controller,
      ran on an allowed node, at exactly C (tree T, nothing dirty), with
      exactly this argv; every fetched file's bytes equal the signed
      receipt's sha256 and the job's build-info.json; build-info names
      this version, commit and tree. Then the byte checks. Prints a
      verdict (JSON); exit 1 when anything is wrong.

  compute_release.py compare --compute DIR --github DIR [--out report.json]
      Shadow report: each compute-built file vs the GitHub-built file of
      the same name (sha256, size, build markers, and for a differing ELF
      the toolchain lines of its .comment section, so the report names the
      cause). Never fails on a difference (that's the finding); exit 2
      only on bad input.

  compute_release.py elf-comment FILE
      The toolchain lines of an ELF's .comment section (rustc, GCC, ld).

  compute_release.py same-files --a DIR --b DIR --names N1,N2,...
      Dual build (G12): the named files are byte-identical in both dirs.

  compute_release.py dual-nodes --a L1.verdict.json --b V1.verdict.json
      Dual build (G12): the two jobs ran on different nodes, by name and
      by node key (each signed receipt's node_fp). Exit 1 when not.

  compute_release.py evidence-bundle --state DIR --git DIR
      The JSON the signing user's `k2sign compute-evidence` reads (S3b):
      <state>/L1 and V1 receipt.json as fetched, plus L1's commit object
      from the git checkout. The signer trusts none of it unchecked.
"""
import argparse
import hashlib
import json
import os
import re
import sys

ELF_MACHINE = {"x86_64": 62, "aarch64": 183}
TEST_KEY_MARKERS = {
    "k2-root-helper": b"K2_ROOT_HELPER_TEST_KEYS_ENABLED",
    "k2-node": b"K2_NODE_TEST_KEYS_ENABLED",
    "k2-app-engine": b"K2_APP_ENGINE_TEST_KEYS_ENABLED",
}
BUILD_MARKERS = {
    "k2-root-helper": rb"K2_ROOT_HELPER_BUILD v=([0-9.]+) proto=[0-9]+ arch=([a-z0-9_]+);",
    "k2-node": rb"K2_NODE_BUILD v=([0-9.]+) proto=[0-9]+ os=linux arch=([a-z0-9_]+);",
    "k2-app-engine": rb"K2_APP_ENGINE_BUILD v=([0-9.]+) proto=[0-9]+ arch=([a-z0-9_]+);",
}


def canonical(v):
    """Canonical JSON (k2-node-proto canonical.rs): sorted keys, no spaces."""
    if isinstance(v, dict):
        return "{" + ",".join(json.dumps(k, ensure_ascii=False) + ":" + canonical(v[k]) for k in sorted(v)) + "}"
    if isinstance(v, list):
        return "[" + ",".join(canonical(x) for x in v) + "]"
    return json.dumps(v, ensure_ascii=False)


def argv_sha256(argv):
    return hashlib.sha256(canonical(list(argv)).encode("utf-8")).hexdigest()


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def load(path):
    with open(path) as f:
        return json.load(f)


def base_of(name):
    """k2-node-linux-x86_64 → k2-node; k2-root-helper-linux-aarch64 → k2-root-helper."""
    m = re.match(r"^(k2-[a-z-]+?)-linux-(x86_64|aarch64)$", name)
    return (m.group(1), m.group(2)) if m else (None, None)


def elf_machine(data):
    if len(data) < 20 or data[:4] != b"\x7fELF" or data[4] != 2:
        return None
    return int.from_bytes(data[18:20], "little" if data[5] == 1 else "big")


def byte_checks(path, name, version, arch, info, problems):
    """Read-only checks on one fetched file (never executed)."""
    with open(path, "rb") as f:
        data = f.read()
    base, file_arch = base_of(name)
    if base is not None:
        if file_arch != arch:
            problems.append(f"{name}: named for {file_arch}, expected {arch}")
        m = elf_machine(data)
        if m != ELF_MACHINE[arch]:
            problems.append(f"{name}: not a 64-bit {arch} ELF (machine {m})")
        marker = TEST_KEY_MARKERS.get(base)
        if marker and marker in data:
            problems.append(f"{name}: built with test-keys")
        pat = BUILD_MARKERS.get(base)
        if pat:
            found = re.findall(pat, data)
            if len(found) != 1:
                problems.append(f"{name}: {len(found)} build markers, want exactly 1")
            elif found[0][0].decode() != version or found[0][1].decode() != arch:
                problems.append(f"{name}: build marker says v={found[0][0].decode()} arch={found[0][1].decode()}, want v={version} arch={arch}")
        if base == "k2-daemon":
            ptyd = (info or {}).get("ptyd_sha256", "")
            if len(ptyd) != 64 or ptyd.encode() not in data:
                problems.append(f"{name}: does not embed k2-ptyd (P84)")
            exp = (info or {}).get("export_sha256", "")
            if len(exp) != 64 or exp.encode() not in data:
                problems.append(f"{name}: does not embed k2-export (exports vs-live §3.1)")
            oauth = (info or {}).get("oauth")
            if oauth == "baked":
                for env in ("K2_GMAIL_CLIENT_ID", "K2_GMAIL_CLIENT_SECRET"):
                    want = os.environ.get(env, "")
                    if want and want.encode() not in data:
                        problems.append(f"{name}: does not contain this Mac's {env}")
            elif oauth != "placeholder":
                problems.append(f"{name}: build-info says oauth={oauth!r}")
    elif name.endswith(".deb"):
        if not data.startswith(b"!<arch>\ndebian-binary"):
            problems.append(f"{name}: not a .deb")
    elif name.endswith(".rpm"):
        if data[:4] != b"\xed\xab\xee\xdb":
            problems.append(f"{name}: not an rpm")
    elif name.endswith(".AppImage"):
        if elf_machine(data) != ELF_MACHINE["x86_64"] or data[8:11] != b"AI\x02":
            problems.append(f"{name}: not a type-2 x86_64 AppImage")


def check_job(a):
    problems = []
    get = load(a.get)
    rcpt = load(a.receipt)
    fetched = load(a.fetched)
    body = rcpt.get("receipt") or {}
    argv = json.loads(a.argv_json)
    nodes = [n for n in a.nodes.split(",") if n]

    if get.get("state") != "done" or get.get("exit") != 0:
        problems.append(f"job ended {get.get('state')} exit {get.get('exit')} ({get.get('reason')})")
    if get.get("ok") is not True:
        problems.append("job result is not ok")
    if not (get.get("receipt") or {}).get("verified") or rcpt.get("verified") is not True:
        problems.append("receipt signature not verified by the controller")
    if get.get("node") not in nodes:
        problems.append(f"ran on {get.get('node')}, not a release node ({', '.join(nodes)})")
    src = body.get("src") or {}
    if src.get("commit") != a.commit:
        problems.append(f"receipt commit {src.get('commit')} != release commit {a.commit}")
    if src.get("tree") != a.tree:
        problems.append(f"receipt tree {src.get('tree')} != release tree {a.tree}")
    if src.get("dirty_sha256"):
        problems.append("receipt says uncommitted changes ran")
    if get.get("commit") != a.commit or get.get("tree") != a.tree:
        problems.append("the controller's job row names another commit/tree")
    if get.get("argv") != argv:
        problems.append(f"argv {get.get('argv')} != {argv}")
    if body.get("argv_sha256") != argv_sha256(argv):
        problems.append("receipt argv_sha256 is not this command's")
    if body.get("state") != "done" or (body.get("exit") or {}).get("code") != 0:
        problems.append("receipt does not say done/exit 0")

    signed = {x["name"]: x for x in body.get("artifacts") or []}
    files = {}
    for f in fetched.get("files") or []:
        name, path = f["name"], f["path"]
        exists = os.path.isfile(path)
        on_disk = sha256_file(path) if exists else None
        size = os.path.getsize(path) if exists else None
        s = signed.get(name)
        if not exists:
            problems.append(f"{name}: fetched file missing at {path}")
        elif s is None:
            problems.append(f"{name}: fetched but not in the signed receipt")
        elif s["sha256"] != f["sha256"] or s["sha256"] != on_disk or s["bytes"] != size:
            problems.append(f"{name}: bytes don't match the signed receipt")
        files[name] = {"path": path, "sha256": on_disk, "bytes": size}
    for name in signed:
        if name not in files:
            problems.append(f"{name}: in the receipt but not fetched")

    info_name = next((n for n in files if n.endswith("build-info.json")), None)
    info = None
    if info_name is None:
        problems.append("no build-info.json among the artifacts")
    else:
        info = load(files[info_name]["path"])
        prefix = info_name[: -len("build-info.json")]
        for k, want in (("version", a.version), ("commit", a.commit), ("tree", a.tree), ("kind", a.kind)):
            if info.get(k) != want:
                problems.append(f"build-info {k} {info.get(k)!r} != {want!r}")
        if a.arch and info.get("arch") != a.arch:
            problems.append(f"build-info arch {info.get('arch')!r} != {a.arch!r}")
        if a.require_oauth and info.get("oauth") != a.require_oauth:
            problems.append(f"build-info oauth {info.get('oauth')!r}, release needs {a.require_oauth!r}")
        for n, meta in (info.get("files") or {}).items():
            got = files.get(prefix + n)
            if got is None:
                problems.append(f"{n}: listed in build-info but not fetched")
            elif got["sha256"] != meta.get("sha256") or got["bytes"] != meta.get("bytes"):
                problems.append(f"{n}: build-info hash differs from the fetched file")
        for n in info.get("files") or {}:
            got = files.get(prefix + n)
            if got and got["sha256"]:
                byte_checks(got["path"], n, a.version, info.get("arch") or "x86_64", info, problems)
        if not info.get("files"):
            problems.append("build-info lists no files")

    verdict = {
        "label": a.label,
        "ok": not problems,
        "job": get.get("job"),
        "node": get.get("node"),
        "node_fp": body.get("node_fp"),
        "receipt_id": (get.get("receipt") or {}).get("id"),
        "commit": a.commit,
        "tree": a.tree,
        "duration_secs": get.get("durationSecs"),
        "tools": body.get("tools"),
        "oauth": (info or {}).get("oauth"),
        "oauth_microsoft": (info or {}).get("oauth_microsoft"),
        "files": {n: {"sha256": v["sha256"], "bytes": v["bytes"]} for n, v in files.items()},
        "problems": problems,
    }
    print(json.dumps(verdict, indent=2, sort_keys=True))
    return 0 if not problems else 1


def elf_comment(path):
    """The toolchain lines in a 64-bit little-endian ELF's .comment section
    (rustc, GCC, clang, linker), sorted and de-duplicated; [] when the file
    is not such an ELF or has no .comment. Read-only, never executed."""
    with open(path, "rb") as f:
        data = f.read()
    if len(data) < 64 or data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        return []
    shoff = int.from_bytes(data[40:48], "little")
    shentsize = int.from_bytes(data[58:60], "little")
    shnum = int.from_bytes(data[60:62], "little")
    shstrndx = int.from_bytes(data[62:64], "little")
    if shoff == 0 or shentsize < 64 or shstrndx >= shnum or shoff + shnum * shentsize > len(data):
        return []

    def sh(i):
        b = shoff + i * shentsize
        name = int.from_bytes(data[b:b + 4], "little")
        off = int.from_bytes(data[b + 24:b + 32], "little")
        size = int.from_bytes(data[b + 32:b + 40], "little")
        return name, off, size

    _, str_off, str_size = sh(shstrndx)
    names = data[str_off:str_off + str_size]
    for i in range(shnum):
        name, off, size = sh(i)
        end = names.find(b"\0", name)
        if names[name:end] == b".comment":
            raw = data[off:off + size].split(b"\0")
            return sorted({x.decode("utf-8", "replace").strip() for x in raw if x.strip()})
    return []


def markers_of(path):
    with open(path, "rb") as f:
        data = f.read()
    out = []
    for pat in BUILD_MARKERS.values():
        out += [m.group(0).decode() for m in re.finditer(pat, data)]
    return out


def compare(a):
    rows = []
    for dirpath, _, names in os.walk(a.compute):
        oauth = None
        bi = os.path.join(dirpath, "build-info.json")
        if os.path.isfile(bi):
            try:
                oauth = json.load(open(bi)).get("oauth")
            except ValueError:
                oauth = None
        for n in sorted(names):
            if n == "build-info.json" or n.endswith(".sha256") or n == "app-linux.sums":
                continue
            p = os.path.join(dirpath, n)
            g = os.path.join(a.github, n)
            rel = os.path.relpath(dirpath, a.compute)
            row = {"name": n, "job": rel.split(os.sep)[0] if rel != "." else "",
                   "compute_sha256": sha256_file(p), "compute_bytes": os.path.getsize(p)}
            if not os.path.isfile(g):
                row.update(match=None, note="no GitHub asset of that name")
            else:
                row.update(github_sha256=sha256_file(g), github_bytes=os.path.getsize(g))
                row["match"] = row["compute_sha256"] == row["github_sha256"]
                if not row["match"]:
                    cm, gm = markers_of(p), markers_of(g)
                    row["markers_equal"] = cm == gm
                    cc, gc = elf_comment(p), elf_comment(g)
                    if cc or gc:
                        row["toolchain_compute"] = cc
                        row["toolchain_github"] = gc
                        row["toolchain_equal"] = cc == gc
                    if n.startswith("k2-daemon-") and oauth == "placeholder":
                        row["note"] = "bytes differ: expected, this daemon was built with the OAuth placeholder and GitHub bakes the real client (K2_COMPUTE_OAUTH=secret builds it the same way)"
                    elif (cc or gc) and cc != gc:
                        row["note"] = "bytes differ: built with another toolchain (see toolchain_compute vs toolchain_github)"
                    elif n.endswith((".deb", ".rpm", ".AppImage")):
                        row["note"] = "bytes differ: packages carry archive timestamps; not expected to match"
                    else:
                        row["note"] = "bytes differ (same rustc, path remap and OAuth mode are needed; see build-info.json)"
            rows.append(row)
    if not rows:
        sys.stderr.write("compare: no compute-built files under %s\n" % a.compute)
        return 2
    summary = {
        "match": sum(1 for r in rows if r["match"] is True),
        "differ": sum(1 for r in rows if r["match"] is False),
        "github_missing": sum(1 for r in rows if r["match"] is None),
    }
    report = {"files": rows, "summary": summary}
    if a.out:
        with open(a.out, "w") as f:
            json.dump(report, f, indent=2, sort_keys=True)
    for r in rows:
        state = {True: "same", False: "DIFFERS", None: "no github asset"}[r["match"]]
        print("  %-3s %-36s %-15s compute %s… (%d B)%s" % (
            r["job"], r["name"], state, r["compute_sha256"][:12], r["compute_bytes"],
            "  github %s… (%d B)" % (r["github_sha256"][:12], r["github_bytes"]) if "github_sha256" in r else ""))
        if r.get("match") is False and r.get("note"):
            print("      %s" % r["note"])
            if r.get("toolchain_equal") is False:
                print("      compute: %s" % "; ".join(r["toolchain_compute"]))
                print("      github:  %s" % "; ".join(r["toolchain_github"]))
    print("  shadow: %(match)d same, %(differ)d differ, %(github_missing)d without a GitHub asset" % summary)
    return 0


def same_files(a):
    bad = []
    for n in [x for x in a.names.split(",") if x]:
        pa, pb = os.path.join(a.a, n), os.path.join(a.b, n)
        if not (os.path.isfile(pa) and os.path.isfile(pb)):
            bad.append(f"{n}: missing on one side")
        elif sha256_file(pa) != sha256_file(pb):
            bad.append(f"{n}: the two builds differ")
    for b in bad:
        print("  ✗ " + b)
    if not bad:
        print("  ✓ identical in both builds: " + a.names)
    return 1 if bad else 0


def dual_nodes(a):
    """V1 is a second build only from a second node: the two verdicts (L1,
    V1) must name different nodes AND different node keys (node_fp, from
    each signed receipt). A missing fingerprint fails: nothing to compare."""
    va, vb = load(a.a), load(a.b)
    bad = []
    fa, fb = va.get("node_fp"), vb.get("node_fp")
    la, lb = va.get("label") or "L1", vb.get("label") or "V1"
    if not fa or not fb:
        bad.append(f"no node_fp in the {la if not fa else lb} receipt: can't tell the two nodes apart")
    elif fa == fb:
        bad.append(f"{la} and {lb} ran on the same node key ({str(fa)[:16]}…): the dual build needs two nodes")
    if va.get("node") and va.get("node") == vb.get("node"):
        bad.append(f"{la} and {lb} both ran on {va.get('node')}: the dual build needs two nodes")
    for b in bad:
        print("  ✗ " + b)
    if not bad:
        print(f"  ✓ {la} on {va.get('node')} and {lb} on {vb.get('node')}: two nodes ({str(fa)[:12]}… / {str(fb)[:12]}…)")
    return 1 if bad else 0


def evidence_bundle(a):
    """The bundle the signing user's `k2sign compute-evidence` reads: L1's
    and V1's signed receipts exactly as `k2 compute receipt --json` printed
    them, and the git commit object L1 built (release.sh's preview commit:
    the tag's tree on the approved base). Nothing here is trusted by the
    signer: it checks the node signatures and hashes the commit itself."""
    recs = {}
    for label in ("L1", "V1"):
        p = os.path.join(a.state, label, "receipt.json")
        if os.path.isfile(p):
            recs[label] = load(p)
    if "L1" not in recs:
        sys.stderr.write("evidence-bundle: no L1/receipt.json in %s\n" % a.state)
        return 1
    r = recs["L1"]
    body = json.loads(r["body"]) if isinstance(r.get("body"), str) else (r.get("receipt") or {})
    commit = (body.get("src") or {}).get("commit") or ""
    if not re.match(r"^[0-9a-f]{40}$", commit):
        sys.stderr.write("evidence-bundle: L1's receipt names no commit\n")
        return 1
    import base64
    import subprocess
    p = subprocess.run(["git", "-C", a.git, "cat-file", "commit", commit], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if p.returncode != 0:
        sys.stderr.write("evidence-bundle: git cat-file commit %s in %s failed: %s\n"
                         % (commit[:12], a.git, p.stderr.decode(errors="replace").strip()[:200]))
        return 1
    json.dump({"v": 1, "commit_object": base64.b64encode(p.stdout).decode(), "receipts": recs},
              sys.stdout, sort_keys=True)
    sys.stdout.write("\n")
    return 0


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("argv-sha256")
    s.add_argument("argv", nargs="*")
    s = sub.add_parser("check-job")
    for k in ("label", "get", "receipt", "fetched", "commit", "tree", "argv-json", "nodes", "version", "kind"):
        s.add_argument("--" + k, required=True)
    s.add_argument("--arch")
    s.add_argument("--require-oauth")
    s = sub.add_parser("compare")
    s.add_argument("--compute", required=True)
    s.add_argument("--github", required=True)
    s.add_argument("--out")
    s = sub.add_parser("elf-comment")
    s.add_argument("file")
    s = sub.add_parser("same-files")
    s.add_argument("--a", required=True)
    s.add_argument("--b", required=True)
    s.add_argument("--names", required=True)
    s = sub.add_parser("evidence-bundle")
    s.add_argument("--state", required=True)
    s.add_argument("--git", required=True)
    s = sub.add_parser("dual-nodes")
    s.add_argument("--a", required=True)
    s.add_argument("--b", required=True)
    a = p.parse_args()
    if a.cmd == "dual-nodes":
        return dual_nodes(a)
    if a.cmd == "evidence-bundle":
        return evidence_bundle(a)
    if a.cmd == "argv-sha256":
        print(argv_sha256(a.argv))
        return 0
    if a.cmd == "check-job":
        if a.kind not in ("linux-bins", "linux-app"):
            sys.stderr.write("--kind is linux-bins or linux-app\n")
            return 2
        return check_job(a)
    if a.cmd == "compare":
        return compare(a)
    if a.cmd == "elf-comment":
        for line in elf_comment(a.file):
            print(line)
        return 0
    return same_files(a)


if __name__ == "__main__":
    sys.exit(main())
