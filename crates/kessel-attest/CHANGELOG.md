# Changelog

## 0.1.0 (unreleased)

First version: `kessel-attest verify` and the library behind it.

- Reads build statements (in-toto Statement v1, SLSA Provenance v1, build
  type `…/compute/buildtypes/job/v1`) in DSSE envelopes, from a
  `.intoto.jsonl` file or an evidence folder.
- Checks, in order: the owner's roster against a pinned roster key; the node
  key in it; the DSSE signature (ECDSA P-256) and the statement's canonical
  form and shape; the owner and key the statement names; the key's status
  and dates; the node's self-statement; the repository, commit and isolation
  asked for; each file's SHA-256; a twin build on another machine.
- A statement from a key that is no longer active is "verified, time not
  proven" (exit 3) unless `--existed-before` (or `--as-of`) gives a time,
  vouched for by the caller and not checked, by which it existed while the
  key was valid. Warns when the roster predates the build or is over 30 days
  old.
- Exit 2 for bad usage or an unreadable input, including no arguments at
  all; with `--json`, that error is JSON on stdout too. `verify --help`
  prints the help.
- ECDSA P-256 through p256 0.14.
