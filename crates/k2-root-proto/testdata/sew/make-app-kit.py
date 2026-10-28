#!/usr/bin/env python3
"""Make K2's fixture Sew App kit, sew-app-kit-0.0.2.tar.gz (0.46.6).

The same shape as Sew's real kit (REL-050): five npm-pack tarballs
(@sew/client, @sew/voice, @sew/channel, @sew/e2ee, @sew/bff), SHA256SUMS,
NOTICES and kit.json, under one top folder. Each package holds a
package.json with the real kit's dependency shape (kit-internal deps exact,
the real external deps and peers) and a tiny index.js; never Sew code.

Deterministic: ustar, root-owned, mode 0644, npm's mtime for the package
tarballs and 0 for the kit, gzip mtime 0. Run from anywhere:

    python3 -I crates/k2-root-proto/testdata/sew/make-app-kit.py

then put the printed sha256 in k2_root_proto::sew::TEST_APP_KITS.
"""
import gzip, hashlib, io, json, os, sys, tarfile

V = "0.0.2"
NPM_MTIME = 499162500
HERE = os.path.dirname(os.path.abspath(__file__))

PACKAGES = {
    "@sew/client": {"dependencies": {}, "peerDependencies": {}},
    "@sew/e2ee": {"dependencies": {}, "peerDependencies": {"livekit-client": "2.22.3"}},
    "@sew/voice": {
        "dependencies": {"@sew/client": V, "@sew/e2ee": V, "lucide-react": "1.48.0"},
        "peerDependencies": {"react": "^19", "react-dom": "^19", "livekit-client": "2.22.3"},
    },
    "@sew/channel": {
        "dependencies": {"@sew/client": V, "@tanstack/react-virtual": "3.14.13", "emojibase-data": "17.0.0", "lucide-react": "1.48.0"},
        "peerDependencies": {"react": "^19", "react-dom": "^19"},
    },
    "@sew/bff": {"dependencies": {"@sew/client": V, "undici": "8.11.2"}, "peerDependencies": {}},
}


def tgz(members, mtime):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as t:
        for name, data in members:
            ti = tarfile.TarInfo(name)
            ti.size = len(data)
            ti.mode = 0o644
            ti.mtime = mtime
            ti.uid = ti.gid = 0
            ti.uname = ti.gname = ""
            t.addfile(ti, io.BytesIO(data))
    out = io.BytesIO()
    with gzip.GzipFile(fileobj=out, mode="wb", mtime=0, filename="") as g:
        g.write(raw.getvalue())
    return out.getvalue()


def sha(b):
    return hashlib.sha256(b).hexdigest()


files, rows = [], []
for name, deps in PACKAGES.items():
    short = name.split("/")[1]
    pj = {"name": name, "version": V, "license": "Apache-2.0", "type": "module", "exports": "./index.js"}
    if deps["dependencies"]:
        pj["dependencies"] = deps["dependencies"]
    if deps["peerDependencies"]:
        pj["peerDependencies"] = deps["peerDependencies"]
    imports = "".join(f'import "{d}";\n' for d in deps["dependencies"] if d.startswith("@sew/"))
    js = f'{imports}export const name = "{name}";\n'.encode()
    b = tgz([("package/package.json", (json.dumps(pj, indent=2) + "\n").encode()), ("package/index.js", js)], NPM_MTIME)
    fname = f"sew-{short}-{V}.tgz"
    files.append((fname, b))
    rows.append({"name": name, "version": V, "file": fname, "sha256": sha(b)})

kit_json = {
    "kit": "sew-app-kit",
    "version": V,
    "packages": rows,
    "external": {"undici": "8.11.2", "@tanstack/react-virtual": "3.14.13", "emojibase-data": "17.0.0", "lucide-react": "1.48.0"},
    "peers": {"react": "^19", "react-dom": "^19", "livekit-client": "2.22.3"},
}
sums = "".join(f"{sha(b)}  {n}\n" for n, b in files)
notices = b"K2 fixture: not Sew. The real kit's NOTICES lists each package's licence (Apache-2.0).\n"
top = f"sew-app-kit-{V}"
members = [(f"{top}/{n}", b) for n, b in files]
members += [
    (f"{top}/SHA256SUMS", sums.encode()),
    (f"{top}/NOTICES", notices),
    (f"{top}/kit.json", (json.dumps(kit_json, indent=2) + "\n").encode()),
]
kit = tgz(members, 0)
out = os.path.join(HERE, f"sew-app-kit-{V}.tar.gz")
with open(out, "wb") as f:
    f.write(kit)
print(f"{sha(kit)}  {os.path.basename(out)}")
