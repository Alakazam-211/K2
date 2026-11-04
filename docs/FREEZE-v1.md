# Format v1 freeze

What this version's evidence is, and how to read it. Results are filled in by a docs-only commit on
top of the commit they ran at; that commit is the one to tag.

## The two commits

- **Evidence commit:** `44c630dcbce4df070a77f11c8e030e13483bfd2a`. Every run named below is at this commit, except where it says otherwise.
- **Tag commit:** the commit holding this text with its results filled in. It differs from the
  evidence commit by this file only: `git diff --stat <evidence> <tag>` shows it.

## How the mutant total is measured

Each mutant removes one check, and the tests must fail. Under these rules, all enforced by the sweep:

- a mutant whose pattern doesn't match the source fails the sweep (`stale`);
- a mutant that doesn't build fails the sweep (`build_failed`);
- a catch must carry an assertion line in a test; a failure with none fails the sweep (`no_assert`);
- a survivor fails the sweep, unless it's listed below with why;
- only a mutant listed below may end in a signal, and it's counted apart.

**This total is a different measure from the earlier sweep's `caught 271`** (at d0af66d), which
counted any non-zero exit as a catch, build failures included, and passed over patterns that no
longer matched. Don't compare the two as one number.

**Result, run `a64c0005`, exit 0, 66 minutes:**

```
mutants result: total=341 caught=338 survived=0 equivalent=2 stale=0 build_failed=0 no_assert=0 aborts_expected=1
needs a look: []
```

The 341 cover the codec, the number rule in Rust and in `vectors/check.mjs`, `apply_group`, the bad-id list and the host-forms check.

## Survivors that are equivalent

- `central-past-end`: with the check gone, the read that follows fails on the same bytes with the
  same `zip_malformed`. The check only avoids allocating first.
- `version-floor`: `VERSION` and `MIN_READER` are both 1, so `min_reader` already holds the floor.
  This stops being equivalent when they differ.

## Ended by a signal, as expected

- `prop-depth`: with the depth bound gone, 5,000,000 open brackets end the process on its stack,
  with no assertion to read. That's what the bound is for, so this mutant shows the bound matters,
  and isn't counted as a catch. The limit itself is held at an assert: the vectors `depth-65` and
  `depth-limit`, through `tests/vectors.rs`, by the mutants `json-depth` and `limit-depth-raised`;
  and `nesting_past_the_limit_is_refused_not_followed` in `tests/decoder_property.rs` on the real code.

## Failing-first

- The codec's first write has mutants as its evidence, with no red commits.
- Everything after it went red first, at its asserted step, then the fix:
  - revisions, groups, dangling blobs, code points, versions, person kinds: red `eeb8bef1` at
    6f2ad65, fix d0af66d;
  - the number rule: red `82e22b05` at 94db6f7, fix 51d8d8c;
  - naming every bad id, and the host-forms check: red `77b9f433` at a6add9d, fix b339a25.
- Tests that closed a gap a sweep found have their red in that sweep's mutant.
- `bad-ids-long-twins` passed when it was added: ids were already told apart by full text. Its red is
  the mutants `bad-once-each-cut` and `js-bad-once-each-cut`.

## Named runs at the evidence commit

- **Repo checks, run `7c92920b`, exit 0:** `repo checks: ok` (formatting, lints, tests, licences, internal hostnames, Windows names, host forms, no kit dependency); `check-windows-names: clean (428 files checked)`; `check-host-forms: clean (428 files checked)`; `check.mjs: passed=56 failed=0 other-layers=284`; Python's `zipfile` and `unzip -t` read a file we wrote, with no errors.
- **Mutant sweep, run `a64c0005`:** above.

**At an earlier commit, d0af66d:** save cost, run `81d75182`, exit 0. Nothing since then changed how a file is written, but the number rule and the bad-id list have changed what's read. It wasn't repeated.

## Not measured

- Save cost on a Mac, on a network share, and on a cold file. The numbers are on one Linux machine.
