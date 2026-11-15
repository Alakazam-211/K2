# Type pack template

A type pack makes one file type multiplayer on this kit. It is its own repo, checked out beside
`k2-mpc-base`, and holds only what is specific to its type:

| Part | What | Required test |
|---|---|---|
| **Schema** | Roots on the kit's primitives (`Schema`, `Kind`) | A `Doc` builds from it |
| **Codec** | File bytes <-> doc state, with limits and decline rules | **Fidelity:** a corpus round-trips byte-exact through doc + store; a small edit changes only what it edits; a carrier pack: the kit's byte-exact row, `test_support::carrier_judge` over the corpus (below) |
| **Verb map + parity list** | `docs/PARITY.md`: every person action -> a verb -> kit API, or N/A with a reason; the same list as data in the crate | `tests/agent_parity.rs`: `k2_mpc_base::parity::check` over every row |
| **ID table** | `Pack::ids()`: every native ID kind its codec and own verbs read, as data (class, scope, the verbs that accept it), and `docs/IDS.md` generated from it with `k2_mpc_base::ids::markdown` (the kit's own kinds: the kit's `docs/IDS.md`) | `ids::check` against `Registry::for_pack`; `docs/IDS.md` equals the generated text; **ID churn:** `ids::churn::judge` (feature `test-support`) over the corpus, zero churn |
| **Editor binding** | Built on the kit's JS client and binding adapters | — |

## The parity list and the pack's own verbs

Every pack declares its parity list **twice, kept in step**: as a table in `docs/PARITY.md`
for people, and as data for the harness. The data is one exhaustive `match`, so a new
`PersonAction` in the kit breaks the pack's build until the pack maps it:

```rust
use k2_mpc_base::parity::{ParityRow, PersonAction};

pub fn parity_row(a: PersonAction) -> ParityRow {
    use PersonAction as P;
    match a {
        P::Open => ParityRow::live(a, &["open"]),
        P::Paste => ParityRow::live(a, &["patch", "apply", "append", "my-verb"]),
        P::Blame => ParityRow::live(a, &["blame"]),
        P::Annotate => ParityRow::deferred(a, &["comment"], "a later release", "why it waits"),
        P::Export => ParityRow::not_applicable(a, &["cat"], &["export"], "why it doesn't apply"),
        // tier 0's rows (the leases): a text pack has no lease
        P::Lease | P::ReleaseLease | P::BreakLease => {
            ParityRow::not_applicable(a, &[], &[], "text docs merge; leases are tier 0's")
        }
        // ... every action, no `_` arm
    }
}

/// The rows of the pack's tier (a text pack: the 31 text rows).
pub fn parity() -> Vec<ParityRow> {
    PersonAction::for_tier(Tier::Text).map(parity_row).collect()
}
```

Each `PersonAction` applies to text docs, blob docs (tier 0) or both (`tiers()`); the
check needs a row for every action of the registry's tier (`Registry::for_pack` is the text
tier) and only checks rows of other tiers if the list has them.

- `live(action, verbs)`: every verb exists and behaves as the person path does;
  `live_but(action, verbs, later)` when some of the row's verbs are still to come;
  `deferred(action, later, until, why)`: the verbs must not exist yet;
  `not_applicable(action, shown_by, absent, reason)`: the type has no such action.
- **The pack's own verbs** (beyond the kit's text tools): implement `Pack::verbs()` with
  `tools::PackVerb::edit(name, usage, serves, exits, parse)`. `parse(args)` refuses bad
  arguments before the doc is touched; the `PackEdit` it returns is a pure function of the
  live text (bounded, no I/O) returning `EditOutput::new(text, stdout, exit)`. The host
  routes the verb through `Session::run_tool`, and `Registry::for_pack` registers it. A
  verb named like a kit verb is refused. Declare its position units with
  `.positions(&[PositionForm::new("--nth", Unit::PackNth)])`, or `.positions(&[])` if it has
  none, and run `parity::positions::check` on the registry in a test (`docs/POSITIONS.md`).
- **`tests/agent_parity.rs`**: `parity::check(pack, &rows, &Registry::for_pack(&pack)?,
  &cases)`, where `cases` holds a `parity::Case` for every pack verb (the person path through
  `Person`, the agent path through `Agent::verb`). Include a negative control: the registry
  `without` the pack's verb must report it missing.

## A format the pack models only in part: the carrier

A pack whose doc holds only part of the file (a deck: slides and shapes; masters, media and
charts opaque) opts in to the **carrier** and writes the file back by splicing, never by
rebuilding it from the doc:

- `Pack::wants_carrier()` returns `true`. The kit then keeps the bytes it last read from the
  file or wrote to it (a blob, with the store's lifecycle: reload, purge, delete) and calls
  `Pack::encode_from(carrier, content, codec)` instead of `encode` wherever it renders the live
  doc as a file.
- `encode_from` decodes `carrier` itself and splices only what differs from `content`, copying
  every other byte. Untouched is byte for byte: when `content` is what `carrier` decodes to, it
  returns `carrier` exactly. The result decodes to `content`. If it can't splice, it returns
  `PackError` (the file is left as it was).
- Keep `encode` working on its own: the kit calls it for the live doc when it holds no carrier
  (a doc never synced with its file). It may return `PackError` when the format can't be built
  from the doc alone. A past version's file (`file_at`) is that version spliced into its own
  carrier; a version without one is refused (`MPC-E208`), never built with `encode`.
- **A carrier pack MUST have the byte-exact row**: an untouched doc writes back byte for byte.
  The kit relies on it: opening an untouched carrier file renders nothing, because the doc as
  imported is taken to render as the file (`docs/SESSIONS.md`, "The carrier"). The row is
  the kit's: a test under `tests/` calls `k2_mpc_base::session::test_support::carrier_judge`
  (feature `test-support`) over the pack's whole corpus and asserts
  `CarrierVerdict::ByteExact`. Per file it checks the hook (`encode_from(file, decode(file))`
  is the file), the file opened live through the kit (it renders as itself, and a flush leaves
  it), and the same after a restart; it reports every failure by file and leg. For a pack that
  doesn't keep a carrier it returns `NotACarrierPack` and checks nothing.
  `scripts/pack-checks/carrier-row.sh` (in the check script below) fails a pack whose
  `wants_carrier` can return `true` and whose tests never call the judge. The pptx pack's
  row that runs the judge over its whole corpus, every fixture stored and deflated and its
  golden deck (`tests/live_carrier.rs`), is a model for a pack's own.
- Also required: a small edit changes only the spliced part (`tests/live_carrier.rs` in the
  kit is a worked example with a toy pack, and runs the judge on it). `docs/SESSIONS.md`, "The
  carrier", has the kit's side.

### A carrier pack that mints its keys: the pairing

A deck's keys are the file's own ids, so `decode(carrier)` gives the live keys back. A pack whose
units have no id in the file (spreadsheet rows, timeline items, scene objects) **mints** its keys:
after a live insert or an app's save moved the units, decoding the carrier mints other keys than
the live doc holds, and the pack can't tell which unit of the file a live key is. Such a pack
opts in to the **pairing** as well:

- `Pack::wants_pairing()` returns `true` (with `wants_carrier()`). The kit then makes a pairing
  at every sync point (a kit write, an outside save taken in, a change that moved no byte, an
  open whose stored pairing doesn't name the sync point), keeps it with every carrier, live and
  per version, and calls `Pack::encode_from_paired(carrier, pairing, content, codec)` instead of
  `encode_from`.
- `Pack::pairing(content, codec)` gives each root's keys in the order the pack's own `decode`
  yields the units of the file `content` renders to. The default, `list_pairing`, is each list
  root's keys in list order; override it when the decode order differs (units across several
  lists, a nested order). **Keys only**: a key must hold no user content (the kit stores it
  beside the carrier and a purge doesn't reach it).
- `encode_from_paired` decodes the carrier, takes **every value from the carrier**, and puts the
  pairing's keys on its units in order. A pairing that doesn't fit (another number of units in a
  root, a root it lacks) returns `SpliceError::PairingMismatch`, and nothing is spliced: **never
  pair by guess** (by position after a change, by value, by the keys `decode` mints). Then it
  diffs that keyed carrier against `content` by key and splices only the diff; `encode_from`'s
  rules stand (untouched is byte for byte, the result decodes to the content).
- Mint the keys an outside save's new units get (`Pack::reconcile`) in a namespace of their own
  (a prefix live inserts never use), so they can't collide with a live insert's key.
- `carrier_judge` checks the paired hook for such a pack (`encode_from_paired` with the pairing of
  `decode(file)`). The kit's `tests/live_paired.rs` is a worked example: a toy pack with rows
  that have no ids, written back after a live insert, an app's save, a restart and a purge, a
  version's file, and a pairing that doesn't fit, refused (`MPC-E521`).
- An outside save on a keyed doc that isn't a table keeps both whenever the doc holds changes
  the file doesn't (or the kit wrote since the last outside save): the save becomes the live doc
  exactly and is never merged by key with unwritten changes, so the pack's rules (no overlap, no
  duplicate names) can't be broken by a merge the pack never saw.

## Rules every pack follows (checked)

- **Engine-free.** A pack never depends on or imports a CRDT engine, in Rust or JS (`yrs`, `yjs`,
  `y-*`, `lib0`, `loro*`, `automerge*`...). It uses the kit's primitives and JS client, so the
  engine can change without changing packs. Enforced by `scripts/pack-checks/engine-free.sh`.
- **Permissive licences only**, checked transitively.
- **Loud tests:** no skips, and no swallowed errors.

## The pack's check script

```sh
#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo test -q --all-targets
./scripts/check-licences.sh
../k2-mpc-base/scripts/pack-checks/engine-free.sh .
../k2-mpc-base/scripts/pack-checks/carrier-row.sh .
```
