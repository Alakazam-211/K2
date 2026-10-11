# pg_query 5.1.1 — K2 path patch

Source: crates.io `pg_query` 5.1.1 (MIT), trimmed to what `build.rs` compiles
(no libpg_query tests/testdata or C++ protobuf output).

Only change: `libpg_query/src/postgres/src_port_snprintf.c` renames the
`strchrnul` fallback to `pg_strchrnul` via a macro (backport of the
PostgreSQL / libpg_query 17-6.1.0 fix), because macOS SDKs from 15.4 on
declare `strchrnul` and the static definition no longer compiles.

Drop this patch when `crates/k2-daemon` moves to pg_query >= 6.1 (PostgreSQL
17 grammar) or a 5.x release carries the fix.
