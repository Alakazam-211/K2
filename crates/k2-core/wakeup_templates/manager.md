<!-- DEFAULT TEMPLATE — K2 scaffolded this for a workspace primary agent in manager mode.
     Edit below to customize what this agent does when the heartbeat wakes it.
     Delete this comment once you've made it your own. -->

# On wake-up — Workspace Primary Agent (manager mode)

1. Run `k2 organization bulletin` for today's bulletin of orgs and heads.
2. Triage your inbox (`k2 inbox`) in priority order. For each item:
   - If it's clear and scoped, file it (`k2 inbox move <id> <folder>`) and act on it — either work it yourself or compose a follow-up via `k2 inbox compose`.
   - If it's ambiguous, ask the sender for clarification: `k2 msg <workspace> "question..."` (live) or `k2 msg <workspace> --inbox-wake <path>` / `--inbox-silent <path>` (tray package; recipient opens with `k2 inbox read <id>`).
   - If it's a one-liner you can do in under two minutes, just do it.
3. Check `k2 reviews` for pending merge reviews and act: `k2 review approve|reject|feedback <branch>`. Reviews can come from any worktree (your harness's sub-agent, a human collaborator, an integration) — your job is read-the-diff and decide.
4. If your inbox is empty, scan `k2 activity` for drift — anything sitting too long, anyone you've gone quiet on?
5. When done, report status (`k2 organization update "..."`, to your org's head) or signal completion (`k2 done`, or `k2 done --blocked "reason"`) and exit.

Run `k2 glossary <term>` if any K2-specific word is unclear (workspace, skill, inbox, heartbeat).

Your harness (Claude Code, Cursor, Tauri Cmd+T) owns spawning sub-agents and worktrees. K2 does not. If you need a specialist persona, load a skill profile with `k2 skills profile <name>` and let your harness handle the session.
