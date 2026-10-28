#!/bin/sh
# A gh stand-in for the k2sign tests (installed as the "pinned" gh in a test
# root only). `gh attestation verify <file> -R <repo> --format json
# [--cert-identity X --source-ref R --source-digest D]` answers from
# $K2SIGN_TEST_FIXTURES/attest/<sha256 of file>.json (the shape gh prints:
# [{verificationResult: {statement: {subject}, signature: {certificate}}}]).
# The real gh does the Sigstore crypto; this stub stands in for it so the
# signer's own logic (which call, which fields, which refusals) is tested.
# Every call is logged to $K2SIGN_TEST_FIXTURES/gh.log.
case "${1:-}" in
    --version) echo "gh version 2.83.1 (k2sign test stub)"; exit 0 ;;
esac
[ "${1:-} ${2:-}" = "attestation verify" ] || { echo "gh-stub: unsupported: $*" >&2; exit 2; }
echo "$*" >> "$K2SIGN_TEST_FIXTURES/gh.log"
[ -n "${GH_TOKEN:-}" ] || { echo "gh-stub: no GH_TOKEN" >&2; exit 4; }
file="$3"; shift 3
ident="" ref="" dig=""
while [ $# -gt 0 ]; do
    case "$1" in
        --cert-identity) ident="$2"; shift ;;
        --source-ref) ref="$2"; shift ;;
        --source-digest) dig="$2"; shift ;;
    esac
    shift
done
sha="$(/usr/bin/shasum -a 256 "$file" | /usr/bin/cut -d' ' -f1)"
f="$K2SIGN_TEST_FIXTURES/attest/$sha.json"
if [ ! -f "$f" ]; then
    echo "✗ Loading attestations from GitHub API failed" >&2
    echo "Error: no attestations found for subject" >&2
    exit 1
fi
if [ -n "$ident$ref$dig" ]; then
    /usr/bin/python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1]))
ident, ref, dig = sys.argv[2:5]
ok = any(c["subjectAlternativeName"] == ident and c["sourceRepositoryRef"] == ref and c["sourceRepositoryDigest"] == dig
         for c in (x["verificationResult"]["signature"]["certificate"] for x in r))
sys.exit(0 if ok else 1)' "$f" "$ident" "$ref" "$dig" || { echo "✗ Verification failed: policy mismatch" >&2; exit 1; }
fi
cat "$f"
