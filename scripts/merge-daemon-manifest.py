#!/usr/bin/env python3
"""Merge the Linux daemon AND App engine builds into daemon-latest.json.

Usage: merge-daemon-manifest.py <manifest.json> <release-download-base> <asset-dir>

Run on z3mbpZ by scripts/sign-daemon.sh after it signs the Linux daemons
(0.46.6, SS1; CI's merge job is gone), for GitHub and K2 Compute builds
alike: keep one copy of these rules. Idempotent: re-runs overwrite the same
keys.

- `linux-<arch>`: k2-daemon-linux-<arch> (url, sig, sha256), as before,
  plus `sig2` (prd-security-sweep-0464-v1 SS4): the URL of the `.sig2`
  made with the K2 update key U2, the only signature 0.46.6+ installs
  accept. `sig` (the retired key U1) stays for older installs; their parsers
  ignore the new field (tests/packaging/daemon-manifest.sh).
- `k2-app-engine-linux-<arch>` (prd-app-runtime-update-continuity-v1 A1,
  R2): the engine build, `Artifact`-shaped so every daemon's parser reads
  it, under its OWN key. Never nested in or beside a "linux-<arch>" key:
  older install-daemon.sh copies sed-parse the LAST url/sha256 after
  `"linux-<arch>"` (tests/packaging/daemon-manifest.sh runs the current and
  the 0.46.3 parser against this output). The engine's `.sig` is published
  at release.sh Step 10.5, up to ~60 min later (A17): the sig URL may 404
  until then, and the daemon's stage and heal handle that.
- An engine binary missing from <asset-dir> is skipped (the daemon heal then
  downloads from the release); airgap assets are refused.
"""

import hashlib
import json
import os
import sys

ARCHES = ("x86_64", "aarch64")


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def entry(base, assets, name):
    return {"url": f"{base}/{name}", "sig": f"{base}/{name}.sig", "sha256": sha256(os.path.join(assets, name))}


def main(argv):
    if len(argv) != 4:
        sys.exit(__doc__)
    manifest_path, base, assets = argv[1], argv[2].rstrip("/"), argv[3]
    with open(manifest_path) as f:
        m = json.load(f)
    arts = m.setdefault("artifacts", {})
    for arch in ARCHES:
        e = entry(base, assets, f"k2-daemon-linux-{arch}")
        e["sig2"] = f"{e['url']}.sig2"
        arts[f"linux-{arch}"] = e
    for arch in ARCHES:
        name = f"k2-app-engine-linux-{arch}"
        if os.path.isfile(os.path.join(assets, name)):
            arts[name] = entry(base, assets, name)
        else:
            print(f"no {name} in {assets}: not listed", file=sys.stderr)
    leaked = [k for k in arts if "airgap" in k]
    if leaked:
        sys.exit(f"daemon-latest.json must not list airgap assets: {leaked}")
    bad = [k for k in arts if k.startswith("linux-") and "engine" in json.dumps(arts[k])]
    if bad:
        sys.exit(f"an engine build sits under a platform key: {bad}")
    with open(manifest_path, "w") as f:
        json.dump(m, f, indent=2)
    print("merged:", sorted(arts))


if __name__ == "__main__":
    main(sys.argv)
