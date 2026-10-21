#!/usr/bin/env bash
# daemon-latest.json generation, sourced by scripts/release.sh (Step 8.5)
# and by tests/packaging/daemon-manifest.sh. Builds no binary, signs and
# uploads nothing: it only prints the manifest JSON.
#
# 0.46.3: the manifest carries `notes` — this version's `## <version>`
# section of WHATS_NEW.md (markdown, header line included) — so a phone or
# remote host can show what's new before updating. The daemon passes it
# through `update/check` as `notes`. A missing section leaves `notes` out
# (release.sh's Step 1.5 already refuses a release without one).
#
#   k2_daemon_manifest <version> <pub_date> <artifacts_fragment> <whats_new.md>
#
# <artifacts_fragment> is the body of the `artifacts` object as release.sh
# builds it (`"macos-aarch64": {...}, "linux-x86_64": {...}`, no braces).
# Prints the manifest on stdout; exits non-zero on bad input.

# Longest notes kept (characters); a longer section is cut at a line end
# with a pointer to the full notes.
K2_DAEMON_MANIFEST_NOTES_MAX="${K2_DAEMON_MANIFEST_NOTES_MAX:-16000}"

k2_daemon_manifest() {
    local version="$1" pub_date="$2" artifacts="$3" whats_new="$4"
    if [ -z "$version" ] || [ -z "$pub_date" ] || [ -z "$artifacts" ]; then
        echo "k2_daemon_manifest: version, pub_date and artifacts are required" >&2
        return 2
    fi
    K2_DM_VERSION="$version" K2_DM_PUB_DATE="$pub_date" K2_DM_ARTIFACTS="$artifacts" \
    K2_DM_WHATS_NEW="$whats_new" K2_DM_NOTES_MAX="$K2_DAEMON_MANIFEST_NOTES_MAX" \
    python3 -I -c '
import json, os, sys

version = os.environ["K2_DM_VERSION"]
artifacts = json.loads("{" + os.environ["K2_DM_ARTIFACTS"] + "}")
if not artifacts:
    sys.exit("k2_daemon_manifest: no artifacts")
limit = int(os.environ["K2_DM_NOTES_MAX"])

notes = None
path = os.environ.get("K2_DM_WHATS_NEW") or ""
if path and os.path.isfile(path):
    head = "## " + version
    out, inside = [], False
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if not inside:
                # "## 0.46.3 — Title" or a bare "## 0.46.3"; never 0.46.30.
                if line == head or line.startswith(head + " "):
                    inside = True
                    out.append(line)
                continue
            if line.startswith("## "):
                break
            out.append(line)
    text = "\n".join(out).strip()
    if text:
        if len(text) > limit:
            cut = text[:limit]
            if "\n" in cut:
                cut = cut[: cut.rfind("\n")]
            text = cut.rstrip() + "\n\n(More in the full release notes.)"
        notes = text

manifest = {"version": version, "pub_date": os.environ["K2_DM_PUB_DATE"], "artifacts": artifacts}
if notes is not None:
    manifest["notes"] = notes
print(json.dumps(manifest, indent=2, ensure_ascii=False))
'
}
