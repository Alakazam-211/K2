# Evidence: the blind check of the `write --base` conflict (v0.9.7.1)

v0.9.7.1 makes `write --base` merge by whole lines, so two writers changing the same line
now conflict, and the second one must re-read and retry. K2 asked that the refusal alone be
enough for an agent to do that. This note records the check: the refusal, captured as an
agent sees it, given with no other context to fresh agents.

## Setup

- **The refusal:** from `mpc`, the kit's reference host (`crates/base/examples/mpc.rs`),
  scenario `write-conflict`, built from release/v0.9.7.1 at ebbbe2c on a build node. The
  agent read version 1 of `notes.md`; Sam then rewrote line 3 (`Status: draft` to
  `Status: in review (Sam)`), making version 2; the agent wrote `--base 1` with line 3 as
  `Status: ready` and line 4 as `Owner: agent`. The host prints `refusal_at(base, current)`.
- **The agents:** two fresh subagents (general-purpose), one for the plain-text refusal and
  one for `--json`. Each prompt held only the refusal and the question below, and asked for
  an answer from the response alone (no commands, no files). They had no repository task,
  no `--help`, no `explain` and no `capabilities`.
- **Pass:** the agent re-reads the current version and retries the write against it, without
  help.

## The refusal (as captured)

`mpc notes.md log` before the write: change 2 (Sam's edit), version 1 at change 1, change 1
(the import); `mpc notes.md status`: `notes.md v2`.

Plain text (stderr, exit 1):

````text
mpc: your write touches lines someone else changed since version 1: 1 region(s), your line 3 (now line 3 of version 2); nothing applied. To retry: read the current text again (cat; it is version 2), redo your change on it, then write --base 2 (MPC-E111)
````

`--json` (stdout, exit 1):

````text
{"id":"MPC-E111","exit":1,"message":"your write touches lines someone else changed since version 1: 1 region(s), your line 3 (now line 3 of version 2); nothing applied. To retry: read the current text again (cat; it is version 2), redo your change on it, then write --base 2","next":"Someone else changed the same lines after the version you read (or added lines at the same place), so nothing was applied; a write never merges inside a line. Run cat to read the current text (the message and details.current give its version), make your change again on that text, and write it with write --base <that version>. Or change only the lines you need with replace --lines or patch. details.conflicts has each region's your_lines (in your text), current_lines (in the current text), base, ours and theirs.","rule":"K-BR-2, K-BR-3 (O1–O5)","details":{"base":1,"current":2,"conflicts":[{"lines":[3,3],"your_lines":[3,3],"current_lines":[3,3],"base":"Status: draft\n","ours":"Status: ready\n","theirs":"Status: in review (Sam)\n"}]}}
````

## Run 1: plain text

Prompt, verbatim:

````text
You are an agent editing the file notes.md through a tool. You tried to save your edit and got this response:

mpc: your write touches lines someone else changed since version 1: 1 region(s), your line 3 (now line 3 of version 2); nothing applied. To retry: read the current text again (cat; it is version 2), redo your change on it, then write --base 2 (MPC-E111)

What exactly do you do next? Give the commands.

(Answer from the response alone; do not run any commands or read any files.)
````

Answer, verbatim:

````text
Nothing was saved. Someone else changed line 3 after I read version 1, and my edit touched that same line. So I fetch the new text, redo my change on top of theirs, and send it with base 2.

The error only names the verbs (`cat`, `write --base`), not the full command. I'm using `mpc` as a stand-in below. Use whatever command produced the error, such as `k2 doc`.

1. **Read version 2.**
   ```
   mpc cat notes.md > notes.v2.md
   ```

2. **Redo my change on version 2.** Look at line 3 as it reads now and work out what the other person changed. Then make my change in a way that keeps their edit. I don't paste my old line 3 over theirs, and I leave the rest of version 2 alone. I edit `notes.v2.md` and save it as `notes.mine.md`.

3. **Write it back with base 2.**
   ```
   mpc write notes.md --base 2 < notes.mine.md
   ```

4. **If it's refused again** because there's now a version 3, I repeat the steps: `cat`, redo the change on that version, then `write --base 3`.

I don't:
- send the same write with `--base 1` again;
- leave out `--base` or force the write, because that would erase the other person's change;
- assume part of my first write was applied. The message says "nothing applied".
````

## Run 2: `--json`

Prompt, verbatim:

````text
You are an agent editing the file notes.md through a tool. You tried to save your edit and got this response:

{"id":"MPC-E111","exit":1,"message":"your write touches lines someone else changed since version 1: 1 region(s), your line 3 (now line 3 of version 2); nothing applied. To retry: read the current text again (cat; it is version 2), redo your change on it, then write --base 2","next":"Someone else changed the same lines after the version you read (or added lines at the same place), so nothing was applied; a write never merges inside a line. Run cat to read the current text (the message and details.current give its version), make your change again on that text, and write it with write --base <that version>. Or change only the lines you need with replace --lines or patch. details.conflicts has each region's your_lines (in your text), current_lines (in the current text), base, ours and theirs.","rule":"K-BR-2, K-BR-3 (O1–O5)","details":{"base":1,"current":2,"conflicts":[{"lines":[3,3],"your_lines":[3,3],"current_lines":[3,3],"base":"Status: draft\n","ours":"Status: ready\n","theirs":"Status: in review (Sam)\n"}]}}

What exactly do you do next? Give the commands.

(Answer from the response alone; do not run any commands or read any files.)
````

Answer, verbatim:

````text
Nothing was saved, and the two edits clash on meaning, not only on text. Sam changed line 3 from `Status: draft` to `Status: in review (Sam)` after I read the file. My edit changes the same line to `Status: ready`. If I just "redo my change on version 2" as the message suggests, I erase Sam's review status without anyone noticing, so I won't do that.

I'm assuming the tool is the `k2 doc` CLI. Use whatever prefix the tool actually has.

**1. Read the current text (version 2)**
```
k2 doc cat notes.md
```
Confirm that line 3 now says `Status: in review (Sam)`, and check whether Sam changed anything else.

**2. Redo my other edits on version 2, keeping Sam's line 3**
Nothing from my write was applied, so any changes I made outside line 3 are lost too and need to be redone. Change only the lines I meant to change, so I can't touch Sam's line again:
```
k2 doc replace notes.md --lines <a>-<b> --base 2 < my-new-lines.txt
```
If I'm rewriting the whole file instead, I build it from the version 2 text with line 3 left as Sam wrote it:
```
k2 doc write notes.md --base 2 < notes-edited-from-v2.md
```
If the status line was my only change, I write nothing.

**3. Settle the status with people, not with a write**
Sam's review comes after my "ready", so moving it to "ready" is Sam's call or my user's. I tell my user something like: "Sam marked notes.md 'in review (Sam)' while I was editing. I left it that way. Should it be 'ready', or should we wait for Sam?" If they say yes, I make that one change with `replace --lines 3-3 --base <current version>`.

**4. If this write is also refused**
If the file has moved past version 2 by then, I go back to step 1 and repeat. I never retry with `--base 1`, and I never try to get around the check.
````

## Verdict

| Run | Refusal | Re-reads the current version | Retries against it | Verdict |
|---|---|---|---|---|
| 1 | plain text | `cat`, version 2 | `write --base 2` with the change redone on version 2; repeats with the new version if refused again | **pass** |
| 2 | `--json` | `cat`, version 2 (from `details.current`) | `write --base 2` from the version 2 text, keeping Sam's line; asks the user about the line both changed | **pass** |

**2 of 2 pass.** Both agents read that nothing was applied, re-read the current version the
refusal named, and wrote again against it; neither retried `--base 1`, forced the write or
dropped Sam's change. Observations:

- Neither agent knew the host's command prefix (the refusal names verbs, not the binary):
  both said so and used a stand-in (`mpc`, `k2 doc`). A host's own `next` text
  (`ErrorReport::with_next`) can name its prefix.
- The `--json` agent used `details.conflicts` to see that the two edits clash in meaning and
  chose to keep Sam's line and ask its user, which `next` allows. In an alternative it wrote
  `replace ... --base 2`; the kit's `replace` takes `--if-version`, not `--base` (its own
  refusal would say so). The write path it chose first is correct.
