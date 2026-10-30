# Changelog

## v0.7.6 (2026-10-29): pairs with kit v0.9.7

**Requires `k2-mpc-base` v0.9.7** (it skips v0.9.6, a one-fix release). Pack changes:
- **Parity row P24 "Export" is live:** `ParityRow::live(a, &["export", "export --version"])`.
  The kit now exports: `export` (needs read) and `export --version V` (needs history) give the
  file through the kit's export registry (as-is in the kit; format exporters are the host's).
  The old N/A row listed `export` as a verb that must not exist, which the kit's registry now
  has, so the parity check would fail against it. `docs/PARITY.md`'s P24 row (the parity test
  checks the table names each verb) and the parity test's counts (28 live rows, no N/A) follow.
- **The demo's golden transcript** (`tests/golden/multiplayer_demo.txt`) changes by one
  line: agent-b's refused `replace --old TODO` (two matches, exit 1) now ends its stderr
  summary with the refusal's stable id, `nothing applied to plan.md · v8 (MPC-E126)`. The kit
  appends the id to an edit verb's "no" since v0.9.7 (`ToolOutput::refusal`); the wording,
  exit code and everything else the demo prints are unchanged (the whole transcript was
  diffed on a build node: that line only, edited by hand, not re-blessed).
- `Cargo.lock`: k2-mpc-base 0.9.7; the crate version. No other pack code change: the kit's
  other v0.9.7 changes (K5 handles in `run_tool` output for App viewers, D1's undo, outside
  saves merged line by line) leave this pack's tests and goldens as they were (checked on a
  build node). No store format change.

The kit's changes since v0.9.5 (its CHANGELOG has the details):
- **v0.9.6:** an outside save that deleted a line and typed it back could leave the line
  twice; the outside-save merge now diffs each side on whole lines.
- **v0.9.7, the Live-tab release:** the live typing APIs (the hunk channel, `edit_at_base`,
  `events_since`, undo over the channel, handles instead of pids in `run_tool` output for App
  viewers, roster push, participant overflow); **D1**: an undo never changes a word someone
  else wrote, and says so instead; speed work (incremental write-back and blame, the
  engine's position index); outside saves merged line by line; exports P0 (`Session::file_at`,
  the export contract, the P24 verbs); stable error ids (`MPC-Ennn`, `docs/ERRORS.md`); host
  guidance per surface, relative paths in refusals and hard-linked files declined (K6, K7,
  K9); the vendored engine at k2-yrs k2.6.3.

## v0.7.5 (2026-10-29)

**Requires `k2-mpc-base` v0.9.5** (a hotfix). No pack code change: `Cargo.lock` and the crate
version only. Nothing to do for this pack; no store format change. The kit's changes:
- **Outside saves:** v0.9.4 narrowed the window in which an outside edit could be corrupted
  after a change that moved no byte; v0.9.5 closes it.
  - A plain-editor save that arrived within the write delay after such a change and an edit
    not yet written back could still land beside the restored text (`LINE three3`).
  - An outside save is now a three-way merge onto the live doc: `write --base`'s merge when
    clean.
  - Where the save overlaps an unwritten change, both are kept, as before, now as whole
    lines and in a deterministic order (the live side first).
- **Echo detection:** a person's save that recreated the bytes of an earlier kit write is no
  longer discarded once they have saved in between. Suppressed echoes are reported
  (`AuditEvent::EchoSuppressed`).
- **Hardening:** client updates carrying subdocuments are refused (the kit never uses them),
  with `DocError::Unaccepted`. A stored doc holding one is quarantined
  (`DocError::Quarantined`). The vendored engine is updated to k2-yrs k2.2.2.
- K-AUD-1 gains `SyncDrift::Pending`.

## v0.7.4 (2026-10-29)

**Requires `k2-mpc-base` v0.9.4** (a hotfix). No pack code change: `Cargo.lock` and the crate
version only. The kit fixes an outside edit that could be corrupted after a change that moved
no byte (since v0.1.0): an edit and its undo, a delete and a retype, or a rewrite and back,
then a plain-editor edit of the same text, could land beside the restored copy (`LINE
three3`) while the doc reported clean. The kit now keeps its disk replica on the doc's own
elements, and adds `Session::audit_disk_sync()` (K-AUD-1's sync check). Nothing to do for
this pack; no store format change.

## v0.7.3 (2026-10-28)

**Requires `k2-mpc-base` v0.9.3.** No pack code change: `Cargo.lock` and the crate version
only. The kit now makes every edit on the doc itself, as the participant's client (no
throwaway replica per edit: byte-identical results, far cheaper as history grows), with a
sampled runtime check against the old path (`Config::edit_check_every`, two new
`AuditEvent` variants). It builds its engine from `vendor/yrs` (Yrs v0.28.0 plus the fork's
patches, MIT) by path, so the lock's `yrs` entry loses its registry source; nothing else
changes for this pack. No store format change.

## v0.7.2 (2026-10-28)

**Requires `k2-mpc-base` v0.9.2** (additive: tier 0 → text promotion keeping history,
`approvals --all`, claims that never overlap at a shared edge, the engine's text cache; no
store format change). No pack code change: the P30 row picks up the kit's `--all` cases with
no list change. `scripts/check.sh` now checks this crate's formatting only (`cargo fmt -p
k2-mpc-markdown`), so the kit's own style and its vendored model don't fail the pack's check.

## v0.7.1 (2026-10-28)

**Requires `k2-mpc-base` v0.9.1** (additive: `edit_with_options`, the `run_tool` flag, the
`approvals` alias of `log --pending`, one BOM on `write`; fixes for blame's `restored by` on a
partly restored line and for last-edit notes). No pack code change: the double-BOM fix comes
through the kit's `TextFilePack`, and the P30 row may leave the `approvals` alias out. The
demo's transcript now shows both demo findings fixed (`restored by` on scenes 6 and 8; no
`edited lines 25–26` notes), and the demo fails if a partly restored line lacks it.

**A multiplayer demo** (`docs/DEMO.md`): three agents and one person on one Markdown doc through the kit's sessions, headless and deterministic. It covers presence notes; a claim, a bounce and a retry with `--if-version`; a person never refused; a `write --base` merge and a conflict; exact `replace`; undo with provenance; blame with `restored by`; and a revert. The example is `crates/markdown/examples/multiplayer_demo.rs`. `tests/demo_transcript.rs` compares its transcript with `tests/golden/multiplayer_demo.txt`. `docs/DEMO.md` maps each scene to a host's doc CLI, so it can be replayed as a dogfood script. No pack or API change; no new dependency.

## v0.7.0 (2026-10-28)

**Requires `k2-mpc-base` v0.9.0** (undo, redo and revert never remove another participant's text, and text they put back keeps its provenance: blame names who first wrote it and who put it back; a behaviour change in undo and blame, hence the minor bump). The base's **store format is 5** (an in-place upgrade from 4; an older build refuses it). `Cargo.lock` only; no pack change.

## v0.6.2 (2026-10-28)

**Requires `k2-mpc-base` v0.8.2** (the revert fix: a revert's conflicts no longer change across a host restart). `Cargo.lock` only; no pack change.

## v0.6.1 (2026-10-28)

**Requires `k2-mpc-base` v0.8.1** (the undo fix: undo, redo and revert remove only their own text). `Cargo.lock` only; no pack change.

## v0.6.0 (2026-10-28)

**Requires `k2-mpc-base` v0.8.0** (`--if-version V` on every edit verb; the kit strips it before the pack parses argv, so `replace-section` takes it too). The usage line and `docs/PARITY.md` name `[--if-version V]`; `Cargo.lock` moves to base 0.8.0. No other change.

## v0.5.0 (2026-10-28)

**Requires `k2-mpc-base` v0.7.0** (agent presence). P8 lists `cursor`, `claim`, `claim --renew`, `claim --release`; P6 adds `activity`; `docs/PARITY.md` updated. No other change.

## v0.4.0 (2026-10-28)

**Requires `k2-mpc-base` v0.6.0**, whose `replace` also serves P12 (exact text,
`replace --old TEXT --new TEXT [--all]`). The P12 row lists `replace` beside `sed -i`
(`parity.rs`, `docs/PARITY.md`) and `Cargo.lock` moves to base 0.6.0. No other change.

## v0.3.1 (2026-10-27)

**Requires `k2-mpc-base` v0.5.0** (store format 4; P29/P30 gain blob rows, text packs unaffected). Only `Cargo.lock` and the crate version change; no code change.

## v0.3.0 (2026-10-27)

**Requires `k2-mpc-base` v0.4.0.** The base added tier 0's lease rows (P32-P34) to
`PersonAction`: `parity_row` maps them N/A (a Markdown doc merges; leases are tier 0's) and
`parity()` lists the text tier's actions (`PersonAction::for_tier(Tier::Text)`), so the list,
`docs/PARITY.md` and the tests stay P1-P31. No other change.

## v0.2.0 (2026-10-27)

P22 (blame) is live. **Requires `k2-mpc-base` v0.3.0**, checked out beside this repo
(`../k2-mpc-base`).

- **`parity_row(P22)`** is `live(&["blame"])`, and `docs/PARITY.md` names `blame [-L a,b]`
  on `Session::blame`. The kit's three P22 cases run through `tests/agent_parity.rs` on the
  Markdown pack: the tab's blame gutter against the verb, the lines in view against `-L`, and
  both refused without the history right.
- **Negative control:** v0.1.0's deferred P22 row now fails against base v0.3.0
  (`UnexpectedVerb`, `StaleDeferral`), and the live row fails when `blame` is taken out of
  the registry (`MissingVerb`).
- The deferred rows are now annotations (P23, P27) and purge (P28).

## v0.1.0 (2026-10-27)

First release of the Markdown pack for Multi-Player Cores (MPC). It requires `k2-mpc-base`
v0.1.0, checked out beside this repo (`../k2-mpc-base`).

- **`MarkdownPack` (`PACK`):** one text root over the Markdown source. The codec round-trips
  BOM and CRLF byte for byte, with a 4 MiB limit and decline rules; `.md` and `.markdown`,
  not `.mdx`.
- **`replace-section`:** a Markdown-native verb. It handles ATX and setext headings, ignores
  fenced code and front matter, and supports `--nth` and `--include-heading`.
- **Fidelity corpus:** byte-exact through doc and store; a one-word edit changes only that
  word.
- **`docs/PARITY.md` P1–P31 as data,** with `tests/agent_parity.rs` running the base harness.
  The deferred rows (blame, annotations, purge) are asserted absent.
- **`scripts/check.sh`:** tests, the licence gate, and the base's engine-free check.
