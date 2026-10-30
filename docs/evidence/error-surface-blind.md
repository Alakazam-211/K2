# Evidence: the blind-agent test of the error surface

Condition 4 of the error-surface slice: a fresh agent with no context gets only the CLI's
`--help`, `explain` and `capabilities`, plus a task that fails, and must recover.

## Setup

- **The CLI:** `mpc`, the kit's reference host (`crates/base/examples/mpc.rs`), built from
  this branch. A daemon holds the sessions for one scenario folder (undo history and claims
  live in memory, as in a real host), and each `mpc` call is a client. Refusals print as
  `mpc: <message> (MPC-Ennn)` on stderr, or as the JSON object with `--json`.
- **The agents:** fresh subagents (general-purpose), one per scenario, with no repository
  access. Each prompt held only the CLI's path, the workspace folder and the task, plus these
  rules:
  - learn the CLI only from `--help`, `explain`, `capabilities` and command output;
  - read no other folder, source, docs or `.mpc/`;
  - no web, cargo, git or k2;
  - ordinary shell tools only inside the workspace, and only where the CLI's guidance points
    to them;
  - no retry loops.
- **The record:** each agent reported its transcript. The daemon logged every request with
  its exit code and error id (`.mpc/requests.log`), and the transcripts below were checked
  against those logs. Final file contents were checked by hand.
- **Five scenarios** (the brief asks for at least four):

  | # | Scenario | The other participants | Task |
  |---|---|---|---|
  | 1 | undo that conflicts | the agent set two lines; Sam then rewrote one of them | "undo your change; don't lose Sam's work" |
  | 2 | held approval | the agent, as Sam's assistant, already has 2 pending changes (cap 2) | "add a goal to plan.md" |
  | 3 | a cap hit (edit rate) | `edit_rate_per_s` 1, `edit_burst` 3 | "add 8 items, one command each is fine" |
  | 4 | declined CSV | `data.csv` mixes `\r\n` and `\n` | "add the row carol,42" |
  | 5 | claimed lines | a bot holds a 90 s claim on lines 3–6, "rewriting the Sync section"; the host respects claims for agents | "fix the typo on line 4" |

  The brief's example of a grid cap needs the CSV grid work (C1), which isn't on this base.
  Its id, `MPC-E802`, is reserved. Scenario 3 is the kit's cap that ships today.
  **Pending C1:** a grid-cap scenario (`MPC-E802 at_limit`, with `NeedsBase` `E801` and
  `FacetConflict` `E803` if they fit), run the same way once C1 wires those ids.

## Round 1

| # | Refusal met | `explain` used | Recovery | Verdict |
|---|---|---|---|---|
| 1 | `MPC-E203` (`undo --partial` skipped line 3) | E203, E120 | Picked `undo --partial` from `capabilities`' P26 row before plain `undo`; undid the clean region (Owner); left Sam's line, as E203 says; told the user the Status choice is Sam's | **pass** |
| 2 | `MPC-E130` held: approvals_backlog | E130, E120 | Didn't retry or approve. Listed the pending changes (`log --pending`) and wrote Sam the exact ask: approve or decline changes 2 and 3, then it adds the line | **pass** |
| 3 | `MPC-E901` rate_limited, ×5 (one loop) | E901 | Following E901's "combine your changes into fewer calls", appended the 5 refused items in one call. 8 lines, in order, no duplicates | **pass** |
| 4 | `MPC-E404` declined_mixed_line_endings | E404 | Took E404's first option, ordinary file tools. Appended `carol,42\r\n` with the shell, left the existing bytes alone, and told the user how to make the file live (make the endings consistent) | **pass** |
| 5 | `MPC-E301` claimed (exit 5, time left) | E301, E120 | Waited out the claim, re-read the doc (unchanged), claimed line 4, then `replace --old/--new --if-version v1`. Typo fixed, nothing else changed | **pass** |

**What round 1 found:** three agents ran verbs that `capabilities` listed but `mpc` didn't
route: `diff --version`, `show --version`, `blame`, `activity` (each `MPC-E120`). The kit was
right; the host listed more than it served. Two fixes followed:

- `mpc` now routes those four verbs.
- `mpc` passes `capabilities` only the verbs it routes (`Registry::without`).

`docs/HOST-ERRORS.md` now says a host passes the registries it actually routes. No kit
message was reworded.

## Round 2 (scenarios 1, 2, 5, after the host fix)

| # | Refusal met | `explain` used | Recovery | Verdict |
|---|---|---|---|---|
| 1 | `MPC-E203`; `MPC-E205` (a change number used as a version) | E203, E205 | Saw the original with `show --version 1` and `blame`, then ran `undo --partial`. Owner went back to `tbd` and Sam's Status line was kept. Checked with `diff --version 1` that only Sam's line differs | **pass** |
| 2 | `MPC-E130`; `MPC-E205` | E130 | As in round 1, and `blame` showed which pending change added which line; the message to Sam names both | **pass** |
| 5 | `MPC-E301` (via `--json`: the object, with `details.remaining_ms`) | E301, E304 | Waited out `details.remaining_ms`; `activity` showed the claim expire; re-read (unchanged), then `replace --old/--new --if-version v1`. Warned the user the bot may bring the typo back if it writes from an old copy | **pass** |

## Verdict

**8 of 8 runs pass (5 scenarios, 3 of them run twice).** Every agent found the id in the refusal, ran `explain` on it, and did what `next` says: it waited where it was told to wait, asked the person where only the person can act, and fixed by hand only where it was told it could. None retried in a loop, bypassed a hold, or overwrote someone else's text. One `--json` refusal was parsed and used (`details.remaining_ms`).

**Observations:**
- **No agent hit plain `undo`'s `MPC-E201`.** Both scenario-1 agents read `capabilities`, saw
  P26 ("Undo the regions of my change nobody touched; report the rest") and went straight
  to `undo --partial`. The all-or-nothing conflict is covered by the reachability test
  (`tests/error_ids.rs`), not by this run.
- **Change numbers and version numbers get mixed up.** Agents passed change numbers to
  `diff --version`. `MPC-E205`'s message and next step were enough each time: list them with
  `log`, use a number it prints.
- **The rate limit was learned from the refusal.** Agents learned it from `MPC-E901`, not
  beforehand, even though `capabilities` lists it. Each still recovered in one step.
- **In scenario 4 the agent chose the conservative option.** It left the file non-live, as
  E404 allows, and explained how to make it live, rather than converting line endings the
  user hadn't asked to change.
