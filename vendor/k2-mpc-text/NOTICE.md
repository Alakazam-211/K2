# Third-party notices

This pack builds on [k2-mpc-base](https://github.com/Alakazam-211/k2-mpc-base) (the
multiplayer kit, v0.8.2). The kit has its own NOTICE, covering:

- yrs (MIT);
- regex-automata (MIT OR Apache-2.0);
- redb (MIT OR Apache-2.0);
- thiserror (MIT OR Apache-2.0);
- `unicode-ident`, which is `(MIT OR Apache-2.0) AND Unicode-3.0` and is used only at build
  time. Unicode-3.0 is OSI-approved and permissive.

The pack itself adds no dependencies. Its tests use proptest (MIT OR Apache-2.0).

`scripts/check-licences.sh` checks every dependency, transitively, against an allow-list:
MIT, Apache-2.0, BSD, ISC and Zlib, plus Unicode-3.0.

The files in `crates/text/tests/corpus/` were written for this repository as test data.
