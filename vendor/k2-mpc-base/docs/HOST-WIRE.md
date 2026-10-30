# Host note: the client wire (`untrusted-wire`) and its six gates

The kit can decode CRDT bytes that editors send (sync step 1 and 2, updates): the
`untrusted-wire` cargo feature of `k2-mpc-base`. **It stays off until all six gates below
hold.** Until then, live editors use the hunk channel (`Session::edit_at_base`,
`Session::events_since`, the JS client in `client/`): no client bytes are decoded as CRDT
data on that path.

## The build-time guard (K10, kit v0.9.8)

With the feature on, `k2-mpc-base` does not compile unless the build also sets
`--cfg k2_mpc_untrusted_wire_ack`. The cfg is the host's statement that it has checked
every gate for the kit version it builds. A host sets it only in its own build, and only
then: `RUSTFLAGS="--cfg k2_mpc_untrusted_wire_ack"`, or `[build] rustflags` in its own cargo
config. A host's CI check that the feature is off stays useful beside it.

The kit's repo sets the cfg for builds run inside it (`.cargo/config.toml`): its own tests
exercise the wire code. Cargo reads that file only for builds started inside the repo, so a
host that vendors the kit or depends on it by path never inherits it.
`tools/tests/wire-guard.sh` (suite part `wire-guard`) proves the guard both ways: in the repo
and as a host builds, the feature alone is refused, and it builds with the cfg or without the
feature.

## The six gates

1. **The engine fork's line.** The vendored engine is at the fork's k2.6 line or later, with
   the decoding and relay fixes that line carries (`vendor/yrs/VENDORED` names the fork's
   tag).
2. **The ingress refusal and the strict decoder.** Client bytes pass a strict decoder before
   the engine sees them: update encoding v1 only (never v2 from a client); decode, validate
   and re-encode the canonical update; bounds on size and nesting depth. The ingress refusal
   that ships today stays in front of it.
3. **Per-connection client-ID ownership.** A connection writes only under its own client ID;
   an update carrying another ID is refused.
4. **Relaying original bytes.** What the host relays to other editors is the client's
   original bytes after the check (or a state-vector diff computed by the kit), never an
   update re-encoded from an engine event.
5. **The wire fuzz target.** A fuzz target over the wire path, clean for at least 24
   cumulative hours in the nightly rig, on the kit version being shipped.
6. **The engine owner's sign-off** on the above, for that kit version.

Each kit release's CHANGELOG says which gates it closes. The wire stays off, and the cfg
unset, until a release states that all six hold.
