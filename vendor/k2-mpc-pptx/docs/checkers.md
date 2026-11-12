# External checkers

Every file this pack writes is judged by the two external test checkers the docx, pptx and xlsx
packs share: the Open XML SDK's validator (.NET, MIT) and Apache POI (Java, Apache-2.0, on an
open JDK). They are **private test tooling**: they live in `k2-mpc-ooxml`'s `checkers/`, kept
on its private main line and not shipped (its release tags leave them out, with the
`docs/checkers.md` there that has the pins, the lock and the rules). They run as separate
processes on Linux build machines only, every download hash-pinned (the JDK too), telemetry
off, every home in the run's own temporary folder, never linked, shipped or copied. This repo
holds none of their code.

On a Linux build machine, with a `k2-mpc-ooxml` checkout of its main branch beside this one
(the checker scripts, at the commit holding the pinned crate or later; `run.sh` fetches the
pinned .NET SDK and JDK itself):

```sh
DIR=$(mktemp -d)
PPTX_EMIT_DIR=$DIR cargo test --locked -p k2-mpc-pptx --test emit
CHECKERS_ON_BUILD_NODE=1 sh ../k2-mpc-ooxml/checkers/run.sh "$DIR"
```

`tests/emit.rs` writes the generated fixtures (stored and deflated), each verb's output and
every merged result of the merge matrix, with `expect-pptx.tsv`: each file's slide and picture
counts as this pack reads them, which the package reader must find too. Every file must pass
the validator with no finding. The files are synthetic; the checkers print names, hashes and
error codes, never content.
