# Multiplayer demo

Three agents (`agent-a`, `agent-b`, `agent-c`) and one person (`sam`) edit one Markdown doc,
`plan.md`, through the kit's sessions: presence notes, claims and a bounce, `--if-version`,
`write --base` (a clean merge and a conflict), exact `replace`, undo with provenance, blame and
revert. It is headless and knows no host: the "host" is a few lines in the example (rights from
a table, a clock that moves only when the script moves it, events kept in a list).

- Code: `crates/markdown/examples/multiplayer_demo.rs`.
- Golden transcript: `crates/markdown/tests/golden/multiplayer_demo.txt`.
- Test: `crates/markdown/tests/demo_transcript.rs` runs the script twice, checks the two runs
  match, and compares the transcript with the golden file byte for byte.

## Run it

```sh
cargo run -p k2-mpc-markdown --example multiplayer_demo     # prints the transcript
cargo test -p k2-mpc-markdown --test demo_transcript         # compares it with the golden file
DEMO_BLESS=1 cargo test -p k2-mpc-markdown --test demo_transcript   # rewrites it (review the diff)
```

The example exits 1 with `demo went off script: …`, followed by the transcript up to that
point, when anything differs from the script. That covers an exit code, a refusal that didn't
happen, a participant's line that went missing, a `write --base` conflict or ambiguous
`replace` that applied something, a blame author, or the final text.

**Deterministic.** The host clock starts at 2026-01-01 00:00:00 UTC and moves only by the
script's steps. Names are fixed, and no fence nonce or client ID is printed. Claim ids are
numbered per process, so the test binary holds a single test.

## Reading the transcript

```text
[00:15] agent-b $ doc replace --old '- week 2: engine' --new '- week 2: sync protocol' plan.md
  stderr: error: lines 8–11 claimed by Agent A (agent-a); 28000 ms left
  (bounced: claim-1 by agent-a, lines 8–11, 28000 ms left)
  exit 5
```

- **`[mm:ss] who $ doc <verb> … plan.md`**: one call of a host's doc CLI.
- **`| `**: stdout.
- **`stderr:`**: stderr: the summary line, `note:` lines and `for-edit` lines.
- **`event:`**: a host event (`ClaimTaken`, `ClaimEnded`).
- **`(…)`**: the host's own remark (a bounce's fields, notes held back by the rate limit).
- Lines starting `--` narrate the scene.
- The person's editor actions (a caret, typing, Ctrl+Z) are printed as plain sentences.
- Every edit runs under `ClaimPolicy::Respect`, which a host would pass by default for agents.
  The person's edits run under it too, to show that a person is never refused.

## Scenes

| # | Scene | What it shows | Exits |
|---|---|---|---|
| 1 | Join | 3 agents (CLI, shared connections) and 1 person (a held tab). `who` lists them. | 0 |
| 2 | Presence | a and b read different sections: no notes. c reads across both: two `note: … viewing` lines. c greps 1 s later and Sam's caret note is held back by the rate limit (one batch per 5 s). 5 s later the same grep gets it. `activity` lists the reads, and `who` shows views and the caret. | 0 |
| 3 | Claims, a bounce, `--if-version` | a's `view --for-edit` takes a 30 s claim. b reads at v1, then edits inside the claim and is **bounced** (exit 5: by, lines, time left). Sam types inside the claim and is never refused. a's edit lands and the claim ends (`ClaimEnded … edited; lines 8–12 free; doc at v3`). b's retry with `--if-version 1` is refused (exit 1, `doc changed since 1 (now 3)`). b re-reads (v3), retries with `--if-version 3`, and the edit lands. | 0, 5, 0, 0, 1, 0, 0 |
| 4 | `write --base` | c reads at v4, Sam adds a goal, then c writes its whole copy with `--base 4`: a clean three-way merge, and Sam's line stays. **Conflict:** b claims the Notes line (`--hold`) and edits it. c's write from v6 changes the same line, so it conflicts (exit 1, nothing applied). `conflict_notes` name b's claim (with its note) and b's edit. b releases the claim. | 0, 0, 1 |
| 5 | Exact `replace` | A unique `--old` applies. `--old TODO` matches twice and is refused (exit 1) with the line numbers (`at lines 18, 24`); nothing is applied. | 0, 1 |
| 6 | Undo with provenance | a, b and c each append a line, then undo their own in the order a, c, b: the others' lines survive each undo. a writes a sentence, b edits inside it and undoes, and blame names a as the author. a's own undo still applies. Sam deletes a whole line and presses Ctrl+Z: blame shows `(file) (restored by Sam in 20)`. | 0 |
| 7 | Blame | Per-line authors at the end: the file, Sam, each agent, and one line restored by Sam. | 0 |
| 8 | Revert | Sam, an owner, reverts b's Timeline change (v4) from scene 3. `- week 2: engine` is back, and Sam's line beside it stays. | 0 |

The final text is checked against the script's expected text.

## Replay on a host

The same script, by hand, on a host that embeds the kit and this pack: three agent terminals
and one person in the editor, on a fresh copy of `plan.md` (the text is `INITIAL` in the
example). Below, **the host's doc CLI** is written `<host> doc <verb> … plan.md`. Each agent
runs its own commands. Sam works in the editor.

Times are not replayable exactly: ages (`3 s ago`), time left on a claim, and which notes the
rate limit holds back all depend on real time. Wait about the same gaps (a few seconds) between
steps. The rate-limit step in scene 2 needs two reads within 5 s, then one more after 5 s.
Versions match only on a fresh doc that nobody else touches.

Check that the host gives the agents' edits the claim policy `Respect` by default. That is
what bounces agent-b in scene 3.

**1. Join**
- Every agent: any doc verb joins, for example `<host> doc who plan.md`.
- Sam: open `plan.md` in the editor.
- Expect `who` to list all four.

**2. Presence**
- agent-a: `<host> doc view --lines 3,6 plan.md`.
- agent-b: `<host> doc view --lines 8,11 plan.md`.
- Sam: put the caret on line 15.
- agent-c: `<host> doc view --lines 1,11 plan.md`. Expect two `note: … viewing` lines on stderr.
- agent-c, within 5 s: `<host> doc grep -n TODO plan.md`. Expect no note (held back).
- agent-c, after 5 s: the same grep. Expect `note: Sam (sam) has a caret at line 15`.
- Sam or any agent: `<host> doc activity -n 10 plan.md`, then `<host> doc who plan.md`.

**3. Claims, a bounce, `--if-version`**
- agent-a: `<host> doc view --lines 8,11 --for-edit plan.md`. Expect `note: for-edit: claimed lines 8–11 for 30 s`.
- agent-b: `<host> doc view --lines 10,11 plan.md` (note the version, v1).
- agent-b: `<host> doc replace --old '- week 2: engine' --new '- week 2: sync protocol' plan.md`. Expect **exit 5** (claimed by Agent A, time left).
- Sam: type `- week 1.5: review the spike` on a new line after line 10. Expect it to apply: a person is never refused.
- agent-a: `<host> doc replace --old '- week 1: spike' --new '- week 1: spike (done)' plan.md`. Expect the claim to end; the host tells agent-b the lines are free and the doc's version.
- agent-b: the same replace with `--if-version 1`. Expect **exit 1**, `doc changed since 1 (now 3)`.
- agent-b: `<host> doc view --lines 10,12 plan.md` (v3), then the replace with `--if-version 3`. Expect exit 0.

**4. `write --base`**
- agent-c: `<host> doc cat plan.md > copy.md` (note the version, v4).
- Sam: type `- show agents' presence to people` after line 6.
- agent-c: add `- two agents rewriting one section` after the licence line in `copy.md`, then `<host> doc write --base 4 plan.md < copy.md`. Expect exit 0, with Sam's line kept.
- agent-c: `<host> doc cat plan.md > copy.md` (v6).
- agent-b: `<host> doc claim --lines 23,23 --ttl 120000 --hold --note 'tightening the notes' plan.md`, then `<host> doc replace --old 'Drafted by the team.' --new 'Drafted by the team; reviewed by B.' plan.md`.
- agent-c: change `Drafted by the team.` to `Drafted by the team in 2026.` in `copy.md`, then `<host> doc write --base 6 plan.md < copy.md`. Expect **exit 1**: a conflict naming agent-b's claim and edit, and nothing applied.
- agent-b: `<host> doc claim --release plan.md`.

**5. Exact `replace`**
- agent-a: `<host> doc replace --old 'TODO: measure' --new 'measured: 3x the file size' plan.md`. Expect exit 0.
- agent-b: `<host> doc replace --old TODO --new DONE plan.md`. Expect **exit 1** and `matches 2 times … (at lines 18, 24)`.

**6. Undo with provenance**
- agent-a, then agent-b, then agent-c: `echo '- A: links checked' | <host> doc append plan.md` (B: `- B: spelling checked`, C: `- C: dates checked`).
- agent-a, then agent-c, then agent-b: `<host> doc undo plan.md`. After each, the other agents' lines are still there.
- agent-a: `echo 'Sync runs over one socket per doc.' | <host> doc append plan.md`.
- agent-b: `<host> doc replace --old 'one socket' --new 'a shared socket' plan.md`.
- Sam: `<host> doc blame -L 25,25 plan.md` (Agent B).
- agent-b: `<host> doc undo plan.md`.
- Sam: `<host> doc blame -L 25,25 plan.md`. Expect Agent A, restored by Agent B (F1 below, fixed in base v0.9.1).
- agent-a: `<host> doc undo plan.md`. Expect exit 0, and the sentence gone.
- Sam: delete the line `TODO: link the spike report.` in the editor, then press Ctrl+Z.
- Sam: `<host> doc blame -L 24,24 plan.md`. Expect `(restored by Sam in …)`.

**7. Blame**
- Sam: `<host> doc blame plan.md`.

**8. Revert**
- Sam: `<host> doc log plan.md` to find agent-b's scene-3 change (v4 on a fresh doc), then `<host> doc revert 4 plan.md`.
- Sam: `<host> doc blame -L 13,13 plan.md`.

**TODO for scene 6 (and the restored-by lines in 7 and 8).** Blame by root and
`restored by` need kit **v0.9.0**, which is this pack's **v0.7.0**. On a host still on base
v0.8.x, restored text is blamed on whoever undid it, with no `restored by`:
- Agent B on line 25 after its undo;
- Sam on line 24 after Ctrl+Z, and on line 13 after the revert.

The undo and revert steps should give the same text there. v0.8.x re-diffs strings, and no
text in this script is repeated where it undoes.

Replay scene 6's blame steps only on a host that vendors base v0.9.0 or later. Until then,
record those differences as expected, not as findings.

## Findings from building it

Both are fixed in `k2-mpc-base` v0.9.1 (this pack's v0.7.1); the transcript shows the fixed
behaviour, and the demo now fails if a partly restored line lacks `restored by`.

- **F1 (fixed): a line put back only in part didn't say "restored by".** After agent-b's
  undo in scene 6, part of the sentence (`one`) is text put back by an undo. Blame by root
  rightly names agent-a, but a line went to the **first** of its characters tied on the
  newest root, which wasn't put back, so the line had no `restored_by` (the same for the
  line Sam's revert puts `engine` back into in scene 8). The kit now prefers a restored
  character among tied ones.
- **F2 (fixed): a pure deletion's "last edit" grew over others' new text.** After agent-b's
  and agent-c's undos removed their lines at the end of Notes, agent-a appended a sentence
  there, and the next notes said `Agent B (agent-b) edited lines 25–26` and `Agent C …
  edited lines 25–26`: agent-a's new line plus a line past the end. Last-edit ranges no
  longer expand (claims still do), are dropped once their text is gone, and never point
  past the end.
