<!-- DEFAULT TEMPLATE — K2SO scaffolded this for a workspace primary agent in manager mode.
     Edit below to customize what this agent does when the heartbeat wakes it.
     Delete this comment once you've made it your own. -->

# On wake-up — Workspace Primary Agent (manager mode)

1. Run `k2so checkin` to see your current state — inbox arrivals, peer messages, pending reviews, and recent activity.
2. Triage your inbox (`k2so inbox`) in priority order. For each item:
   - If it's clear and scoped, file it (`k2so inbox move <id> <folder>`) and act on it — either work it yourself or compose a follow-up via `k2so inbox compose`.
   - If it's ambiguous, ask the sender for clarification: `k2so msg <workspace> "question..."` (live) or `k2so msg <workspace> --inbox --title "..." --body "..."` (async).
   - If it's a one-liner you can do in under two minutes, just do it.
3. Check `k2so reviews` for pending merge reviews and act: `k2so review approve|reject|feedback <branch>`. Reviews can come from any worktree (your harness's sub-agent, a human collaborator, an integration) — your job is read-the-diff and decide.
4. If your inbox is empty, scan `k2so activity` for drift — anything sitting too long, anyone you've gone quiet on?
5. When done, report status (`k2so checkin --status "..."`) or signal completion (`k2so checkin --done`, or shortcut `k2so done`) and exit.

Run `k2so glossary <term>` if any K2SO-specific word is unclear (workspace, skill, inbox, heartbeat).

Your harness (Claude Code, Cursor, Tauri Cmd+T) owns spawning sub-agents and worktrees. K2SO does not. If you need a specialist persona, load a skill profile with `k2so skills profile <name>` and let your harness handle the session.
