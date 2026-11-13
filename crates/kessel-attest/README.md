# kessel-attest

Checks a build statement offline: from a build machine owner's key, through
the owner's signed roster of machine keys and the machine's signature, down
to the files you hold. No server, no network, pure Rust.

A **build statement** is an [in-toto Statement v1] with a [SLSA Provenance
v1] predicate, in a [DSSE envelope], signed by the machine (the *node*) that
ran the build, one JSON line per job (`<job label>.intoto.jsonl`, such as
`L1.intoto.jsonl`). It says: these files, with these SHA-256 digests, were
built from this commit with this command, walled off from other jobs this
way. The node's **owner** signs a **roster**, its list of node keys with
each machine's name, host, status and dates. You pin the owner's roster key
fingerprint, got from the owner out of band, and kessel-attest checks
everything else from there. The words are explained under
[Words used here](#words-used-here).

Used in K2 Compute: its servers hand out the statements with a `trust/`
folder (`k2 ci provenance --out DIR`, `k2 ci evidence --out DIR`). The
formats are [docs/provenance-v1.md] and [docs/ci-formats-v1.md] in the K2
repository. Licensed under MIT OR Apache-2.0, at your option.

## Install

```sh
cargo install kessel-attest
```

It is not on crates.io yet; until it is, build it from source:

```sh
git clone https://github.com/Alakazam-211/K2
cargo install --path K2/crates/kessel-attest
```

## Use

```sh
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
With `--json` the result is one JSON object on stdout, and so is an exit-2
error: `{"error": "...", "exitCode": 2}`. `kessel-attest --help` (or
`kessel-attest verify --help`) lists every option and rule in order.

## A worked example

The crate carries one: `tests/fixtures/example/` holds a file, the statement
for its build (`provenance/L1.intoto.jsonl`) and the `trust/` folder that
came with it, all signed with test keys (the owner key is a fixed test key,
and the node key is the golden vectors' `TEST KEY ONLY`). The file is a
stand-in whose bytes are its own name. From a clone:

```sh
cd K2/crates/kessel-attest/tests/fixtures/example
kessel-attest verify acme-linux-x86_64 \
  --provenance provenance/L1.intoto.jsonl --trust trust \
  --owner-key fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889
```

It exits 0 and prints:

```text
verified: acme-linux-x86_64
  ok        roster           roster          acme-owner.example.test's roster seq 1 (issued 2026-10-06T00:00:00Z, 1 keys) verifies with the pinned owner key fe812c12f3ab4ce6…
  ok        L1               node key        node test-node-01 (host host-a), key e9017ba77f8a0fca…, epoch 3, active in the roster
  ok        L1               signature       the DSSE signature verifies with the node key; the statement is well formed
  ok        L1               owner           builder.id names the pinned owner acme-owner.example.test
  ok        L1               key status      node test-node-01's key is active; the job started 2026-10-05T08:12:11Z (node clock)
  ok        L1               self-statement  node test-node-01: wall job_user, key not readable by jobs, taint clean
  ok        acme-linux-x86_64 file            sha256 beab409fa4f6a295… is subject dist/acme-linux-x86_64
L1: built by node test-node-01 (host host-a) of acme-owner.example.test from git+https://git.example.test/acme/app.git at 6a428f7db79583d6a1271691be00a61dd80ff884
  command: bash scripts/ci/build.sh x86_64
  ran 2026-10-05T08:12:11Z to 2026-10-05T08:37:40Z (the node's clock), isolation exclusive, wall job_user, network none
warning: the roster (seq 1) was issued 2026-10-06T00:00:00Z, over 30 days ago: get the owner's current roster and pass it with --roster
```

Line by line: the owner's roster verifies with the key you pinned; the
statement `L1` is signed by a node key that roster lists as active; the
signature and the statement's form hold; the statement names that owner;
the node's own self-statement says its jobs ran walled off and couldn't
read its key; and your file's SHA-256 is one the statement names (as
`dist/acme-linux-x86_64`, the path the build wrote it to). The warning is
about the roster, not the build: see [Revocation](#revocation). Change one
byte of `acme-linux-x86_64` and it exits 1 with `NOT verified: … file: …`.

## What a pass means, and what it doesn't

It means: the owner vouches for a machine, and that machine says it built
these bytes from that commit with that command. It does not prove the owner
honest, the source safe or the build reproducible. A **twin** (`--twin`: a
second build of the same commit on another machine, one with a different
`host` in the roster, with the same digests) narrows what one bad machine
can do; it does nothing against the owner's roster key itself.

## Time

Every time in a statement is the build machine's own clock. For a key that
is still active in the roster that is fine. For a key the roster lists as
retired, whoever holds the old key could sign a new statement and date it
before the retirement, so kessel-attest says **verified, time not proven**
(exit 3).

`--existed-before TIME` (or its other name, `--as-of TIME`) settles it with
a time by which you know the statement existed, from a timestamp or log you
trust, such as the release's own signature date. **kessel-attest doesn't
check that time: it takes your word for it.** A time on or before the
key's retirement makes the verdict `verified`; a later one leaves it at
`verified, time not proven`. A revoked key fails whatever the time.

## Revocation

A `trust/` folder's roster is as of the build: a key revoked later still
looks active in it. kessel-attest warns when the roster is older than the
build or over 30 days old. Pass the owner's current roster with `--roster`;
the newest by `seq` is used (two different rosters with one `seq` are
refused).

Where to get the current one: the owner hands each new roster to its nodes
and to the K2 servers that use them (`k2 compute roster publish`). Running
`k2 ci evidence <run> --out DIR` (or `k2 ci provenance`) again writes the
newest roster your server holds for that owner to
`DIR/trust/rosters/<owner key fingerprint>.json`; pass that file.

## Words used here

- **job label**: the name of one job in a build, such as `L1` (the build) or
  `V1` (a second build to compare). A statement file is
  `<job label>.intoto.jsonl`. Labels name jobs only; a machine has a name
  and a host.
- **node**: a build machine running K2's node agent, which signs the
  statements. Its **name** in the roster stays the same when it gets a new
  key.
- **host**: the owner's word, in the roster, for the physical machine a node
  runs on. Two nodes with the same host count as one machine for `--twin`.
- **keyid**: the node key's fingerprint: SHA-256 of its SubjectPublicKeyInfo
  DER, 64 lowercase hex. It is the envelope's `keyid`, the roster's
  `node_fp`, and the file name in `trust/nodes/<keyid>.pem`.
- **roster**: the owner's signed list of node keys (`{body, sig}`, signed
  with the owner's Ed25519 roster key), one entry per key: the node's name
  and host, the key's epoch, status, dates and taint basis.
- **seq**: the roster's number. It rises by one each time the owner signs a
  new roster; the highest you give is used.
- **epoch**: which key of a node this is: 0 for the key it enrolled with,
  one more each time it gets a new key. The statement, the self-statement
  and the roster must name the same epoch.
- **active, retired, revoked**: a key's status in the roster. *Active*: in
  use. *Retired*: replaced, usually by the node's next key; a statement
  from a job that started on or before the retirement (`valid_to`) still
  counts, but see [Time](#time). *Revoked*: withdrawn by the owner
  (compromised, key lost, or another reason); every statement from it
  fails, whatever its date.
- **self-statement**: the node's own signed description of itself: its key
  and epoch, its job wall, whether it is a `--dev` node, whether jobs could
  read its key, its taint, its versions and OS. It is in
  `trust/statements/<sha256>.json`, and the roster names the one the owner
  approved.
- **job wall**: what keeps a job away from the node agent and its key. On
  Linux each job runs as its own OS user (`job_user`); on macOS, inside the
  system sandbox (`seatbelt`). A job that ran with no wall (`none`) fails.
- **taint, taint_basis**: a key is *tainted* when a job ever started under
  it with no wall; that fails. `unknown` means the key is older than that
  record. The roster's `taint_basis` says why the owner vouches for the
  key: `self_clean` (its self-statement says clean), `rekeyed` (a fresh key
  made since the record existed) or `owner_attested` (the owner checked by
  hand that it never ran a job without the wall; the only basis that
  accepts `unknown`). No basis: the owner vouches for nothing, and it fails.
- **isolation**: how the job was kept apart from other jobs on the node.
  `job_uid`: a fresh OS user per job. `exclusive`: nothing else ran on the
  node. `tenant_uid`: one OS user per tenant, shared by that tenant's other
  jobs. `shared_uid`: no separation between jobs (macOS sandbox nodes and
  `--dev` nodes). `--isolation job_uid,exclusive` accepts only the first
  two.
- **`--dev` node**: a node agent started with `k2-node run --dev`, for an
  owner's local smoke tests: it may run as the person's own user, and its
  jobs can read that user's files. Its statements always fail.

## Stock tools

The envelope and the node key (`trust/nodes/<keyid>.pem`) are standard, so
stock DSSE tools can check the node signature alone; the owner and roster
chain is this tool's. `scripts/stock-verifiers.sh` runs cosign and the
in-toto reference library (securesystemslib) on the golden statement
(`tests/fixtures/provenance-v1.json`) and prints the exact commands that
pass.

## Library

The same check from Rust, on the worked example (paths are relative to the
crate's folder):

```rust,no_run
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use kessel_attest::load;
use kessel_attest::trust::TrustDir;
use kessel_attest::verify::{verify, Input, Policy, Verdict};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new("tests/fixtures/example");
    // The owner's roster key fingerprint, got from the owner, never from trust/.
    let owner_fp = "fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889".to_string();
    // This machine's clock in unix seconds (i64): the roster age warning uses it.
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

    let input = Input {
        statements: load::envelopes(&dir.join("provenance/L1.intoto.jsonl"))?,
        twin: None,
        files: vec![load::file_arg(&dir.join("acme-linux-x86_64"))?],
        trust: Some(TrustDir::open(&dir.join("trust"))?),
        rosters: Vec::new(),
        policy: Policy { owner_fp, ..Policy::default() },
        now,
    };
    let report = verify(&input);
    println!("{}", report.verdict.text());
    assert_eq!(report.verdict, Verdict::Verified);
    Ok(())
}
```

## Dependencies

serde, serde_json, base64, sha2, p256 (ECDSA P-256) and ed25519-dalek, all
pure Rust under MIT, Apache-2.0 or BSD-3-Clause (plus Unicode-3.0 for
unicode-ident, which serde's derive uses at build time). No C, no async
runtime, no network code.

[in-toto Statement v1]: https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md
[SLSA Provenance v1]: https://slsa.dev/spec/v1.0/provenance
[DSSE envelope]: https://github.com/secure-systems-lab/dsse/blob/master/envelope.md
[docs/provenance-v1.md]: https://github.com/Alakazam-211/K2/blob/main/docs/provenance-v1.md
[docs/ci-formats-v1.md]: https://github.com/Alakazam-211/K2/blob/main/docs/ci-formats-v1.md
