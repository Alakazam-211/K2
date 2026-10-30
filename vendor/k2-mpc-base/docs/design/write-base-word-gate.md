# `write --base`: merging inside a line (deferred)

**Status:** not built. `write --base` merges whole lines (v0.9.7.1, kept in v0.9.8): two
changes to one line are a conflict. This note keeps the work on merging inside a line, so it
can come back as its own release with its own evidence. Rule for the kit: whole-file writes
merge whole units (lines, rows); targeted verbs merge finer.

## What was tried: the 1:1:1 rule

Lines are paired first; characters merge only inside a base line each side turned into exactly
one line; anything else (a line deleted, rewritten into several, a last line closed) is whole
lines on both sides. It was built on branch `merge3/v0.9.8` (kept as the record) and withdrawn
before release.

## Why it was withdrawn

Faults it produced or left, each found by a judge or a property within a day:

| Base | One side | Other side | Result | Why |
|---|---|---|---|---|
| `p\nz\n` | `xp\nz\n` | `z\n` | `xz` | an insertion at a line's start only touches the other side's deletion |
| `p\n` | `xp\n` | `q\nr\n` | `xq` | the same, beside a rewrite |
| `p00:0\n` | `x0p00:0` | `x0p00:0\n` | `x0x0p00:0` | the same text typed twice, one side also dropping the final break |
| `alpha\n` | `alXpha\n` | `alphYa\n` | `alXphYa` | two edits inside one word |
| `foo\n` | `x` typed at the start | `foo` replaced by `xfo2` | `xxfo2` (reasoned, not run) | an insertion touching a replacement |

The first three were fixed one at a time; the point is the rate, and that each fix was a
new special case.

## The candidate: a word gate

Characters merge only when the two sides' changes fall in different whitespace-delimited
words of a 1:1:1 line; the same word is a conflict. That makes "different words" literally
true: it keeps `wt wo` and refuses `alXphYa`, `XwordY` and `xxfo2`.

**Why it doesn't close the hole: a moved line can look like an edit.** Base `M`, `Z`, `X` with
ours `X`, `Z`, `M` is, to a line diff, a 1:1 gap `M` → `X`. A word gate then merges the other
side's change to `M` into text that has moved away. Pairing can't tell a move from an edit.

## The judge's independence

A judge for this rule needs two things, and the merged line is the harder one:
- **The pairing** can be independent: the judge's own LCS tables decide that under some
  alignment each side turned that one base line into that one line.
- **The merged text** was not: the judge took it from the kit's single-line merge, so it could
  not reject `alXphYa`. A real judge needs its own statement of what a merged line may be
  (for the word gate: the base's words, with each changed word taken from the side that
  changed it), written without the kit's diff.

## What would bring it back

Evidence that same-line conflicts cost agents enough to pay for this: count, from dogfood, how
often `write --base` is refused where the two sides' changes are in different words of one line,
and how often the retry succeeds. **Proposed threshold, not decided:** a share of `write --base`
calls large enough that retries are a real cost; the owner sets the number when there's data.

If it comes back: its own release, the word gate, a judge with its own merged-line
statement, the moved-line case decided explicitly, and every shape above as a regression.
