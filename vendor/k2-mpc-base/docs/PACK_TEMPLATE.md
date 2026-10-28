# Type pack template

A type pack makes one file type multiplayer on this kit. It is its own repo, checked out beside
`k2-mpc-base`, and holds only what is specific to its type:

| Part | What | Required test |
|---|---|---|
| **Schema** | Roots on the kit's primitives (`Schema`, `Kind`) | A `Doc` builds from it |
| **Codec** | File bytes <-> doc state, with limits and decline rules | **Fidelity:** a corpus round-trips byte-exact through doc + store; a small edit changes only what it edits |
| **Verb map + parity list** | `docs/PARITY.md`: every person action -> a verb -> kit API, or N/A with a reason; the same list as data in the crate | `tests/agent_parity.rs`: `k2_mpc_base::parity::check` over every row |
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
        // ... every action, no `_` arm
    }
}
```

- `live(action, verbs)`: every verb exists and behaves as the person path does;
  `live_but(action, verbs, later)` when some of the row's verbs are still to come;
  `deferred(action, later, until, why)`: the verbs must not exist yet;
  `not_applicable(action, shown_by, absent, reason)`: the type has no such action.
- **The pack's own verbs** (beyond the kit's text tools): implement `Pack::verbs()` with
  `tools::PackVerb::edit(name, usage, serves, exits, parse)`. `parse(args)` refuses bad
  arguments before the doc is touched; the `PackEdit` it returns is a pure function of the
  live text (bounded, no I/O) returning `EditOutput::new(text, stdout, exit)`. The host
  routes the verb through `Session::run_tool`, and `Registry::for_pack` registers it. A
  verb named like a kit verb is refused.
- **`tests/agent_parity.rs`**: `parity::check(pack, &rows, &Registry::for_pack(&pack)?,
  &cases)`, where `cases` holds a `parity::Case` for every pack verb (the person path through
  `Person`, the agent path through `Agent::verb`). Include a negative control: the registry
  `without` the pack's verb must report it missing.

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
