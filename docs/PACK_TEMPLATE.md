# Type pack template

A type pack makes one file type multiplayer on this kit. It is its own repo, checked out beside
`k2-mpc-base`, and holds only what is specific to its type:

| Part | What | Required test |
|---|---|---|
| **Schema** | Roots on the kit's primitives (`Schema`, `Kind`) | A `Doc` builds from it |
| **Codec** | File bytes <-> doc state, with limits and decline rules | **Fidelity:** a corpus round-trips byte-exact through doc + store; a small edit changes only what it edits |
| **Verb map + parity list** | `docs/PARITY.md`: every person action -> a verb -> kit API, or N/A with a reason; the same list as data in the crate | `tests/agent_parity.rs`: `k2_mpc_base::parity::check` over every row |
| **ID table** | `Pack::ids()`: every native ID kind its codec and own verbs read, as data (class, scope, the verbs that accept it; K-ID-13), and `docs/IDS.md` generated from it with `k2_mpc_base::ids::markdown` (the kit's own kinds: the kit's `docs/IDS.md`) | `ids::check` against `Registry::for_pack`; `docs/IDS.md` equals the generated text; **ID churn:** `ids::churn::judge` (feature `test-support`) over the corpus, zero churn (K-COD-10) |
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
        P::Annotate => ParityRow::deferred(a, &["comment"], "M5", "why it waits"),
        P::Export => ParityRow::not_applicable(a, &["cat"], &["export"], "why it doesn't apply"),
        // tier 0's rows (the leases): a text pack has no lease
        P::Lease | P::ReleaseLease | P::BreakLease => {
            ParityRow::not_applicable(a, &[], &[], "text docs merge; leases are tier 0's")
        }
        // ... every action, no `_` arm
    }
}

/// The rows of the pack's tier (a text pack: P1-P31).
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

## A format the pack models only in part: the carrier (K1)

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
- Required tests: **untouched is byte for byte** through `encode_from` over the corpus, and a
  small edit changes only the spliced part (`tests/live_carrier.rs` in the kit is a worked
  example with a toy pack). `docs/SESSIONS.md`, "The carrier (K1)", has the kit's side.

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
```
