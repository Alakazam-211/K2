# Type pack template

A type pack makes one file type multiplayer on this kit. It is its own repo, checked out beside
`k2-mpc-base`, and holds only what is specific to its type:

| Part | What | Required test |
|---|---|---|
| **Schema** | Roots on the kit's primitives (`Schema`, `Kind`) | A `Doc` builds from it |
| **Codec** | File bytes <-> doc state, with limits and decline rules | **Fidelity:** a corpus round-trips byte-exact through doc + store; a small edit changes only what it edits |
| **Verb map + parity list** | `docs/PARITY.md`: every person action -> a verb -> kit API, or N/A with a reason | The parity harness walks the list (kit framework, arriving with sessions) |
| **Editor binding** | Built on the kit's JS client and binding adapters | — |

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
