# kessel-attest

Checks a build statement offline: from a build machine owner's key, through
the owner's signed roster of machine keys and the machine's signature, down
to the files you hold. No server, no network, pure Rust.

A **build statement** is an [in-toto Statement v1] with a [SLSA Provenance
v1] predicate, in a [DSSE envelope], signed by the machine (the *node*) that
ran the build, one JSON line per job (`<label>.intoto.jsonl`). It says: these
files, with these SHA-256 digests, were built from this commit with this
command, walled off from other jobs this way. The node's **owner** signs a
**roster**, its list of node keys with each machine's label, status and
dates. You pin the owner's roster key fingerprint, got from the owner out of
band, and kessel-attest checks everything else from there.

Used in K2 Compute: its servers hand out the statements with a `trust/`
folder (`k2 ci provenance --out DIR`, `k2 ci evidence --out DIR`). The
formats are in `docs/provenance-v1.md` and `docs/ci-formats-v1.md` in the
source repository. Licensed under MIT OR Apache-2.0, at your option.

## Use

```sh
cargo install --path crates/kessel-attest

kessel-attest verify dist/app-linux-x86_64 \
  --provenance provenance/L1.intoto.jsonl --trust trust/ \
  --owner-key <the owner's roster key fingerprint, 64 hex> \
  [--repo git.example.test/acme/app] [--commit <40 hex>] \
  [--isolation job_uid,exclusive] [--twin provenance/V1.intoto.jsonl] \
  [--roster current-roster.json] [--existed-before 2027-01-20T00:00:00Z] [--json]

# Or an evidence folder: provenance/, trust/ and the fetched job files.
kessel-attest verify --dir evidence/ --owner-key <fp>
```

Exit codes: `0` verified; `1` not verified (the failed rule is named); `2`
bad usage or an unreadable input; `3` **verified, time not proven** (below).
`kessel-attest --help` lists every rule in order.

## What a pass means, and what it doesn't

It means: the owner vouches for a machine, and that machine says it built
these bytes from that commit with that command. It does not prove the owner
honest, the source safe or the build reproducible. A **twin** (`--twin`: a
second build of the same commit on another machine, by the roster's `host`
label, with the same digests) narrows what one bad machine can do; it does
nothing against the owner's roster key itself.

**Time.** Every time in a statement is the build machine's own clock. For a
key that is still active in the roster that is fine. For a key the roster
lists as retired, whoever holds the old key could sign a new statement and
date it before the retirement, so kessel-attest says **verified, time not
proven** (exit 3) unless you pass `--existed-before` with proof of when the
file existed, such as the release's own signature date. A revoked key fails
whatever the time.

**Revocation.** A `trust/` folder's roster is as of the build: a key revoked
later still looks active in it. kessel-attest warns when the roster is older
than the build or over 30 days old; pass the owner's current roster with
`--roster`, and the newest by `seq` is used (two different rosters with one
`seq` are refused).

## Stock tools

The envelope and the node key (`trust/nodes/<keyid>.pem`) are standard, so
stock DSSE tools can check the node signature alone; the owner and roster
chain is this tool's. `scripts/stock-verifiers.sh` runs cosign and the
in-toto reference library (securesystemslib) on the golden statement and
prints the exact commands that pass.

## Library

```rust,ignore
use kessel_attest::{load, trust::TrustDir, verify::{verify, Input, Policy}};

let input = Input {
    statements: load::envelopes("provenance/L1.intoto.jsonl".as_ref())?,
    twin: None,
    files: vec![load::file_arg("dist/app".as_ref())?],
    trust: Some(TrustDir::open("trust".as_ref())?),
    rosters: Vec::new(),
    policy: Policy { owner_fp: owner_fp.into(), ..Policy::default() },
    now: unix_now,
};
let report = verify(&input);
println!("{}", report.verdict.text());
```

## Dependencies

serde, serde_json, base64, sha2, p256 (ECDSA P-256) and ed25519-dalek, all
pure Rust under MIT, Apache-2.0 or BSD-3-Clause. No C, no async runtime, no
network code.

[in-toto Statement v1]: https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md
[SLSA Provenance v1]: https://slsa.dev/spec/v1.0/provenance
[DSSE envelope]: https://github.com/secure-systems-lab/dsse/blob/master/envelope.md
