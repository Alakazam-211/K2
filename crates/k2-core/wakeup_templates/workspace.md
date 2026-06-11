<!-- DEFAULT TEMPLATE — K2SO scaffolded this for the workspace's primary agent in manager mode.
     Edit below to customize what the agent does when the heartbeat wakes it.
     Delete this comment once you've made it your own. -->

# On wake-up — Workspace Primary Agent (manager mode)

1. Run `k2so checkin` to see new arrivals: inbox items, peer messages, pending reviews, recent activity.
2. Triage your inbox (`k2so inbox`). For each item, decide: act on it yourself, file it for later (`k2so inbox move <id> <folder>`), or — if it should go elsewhere — forward via `k2so msg <workspace> --inbox --title "..." --body "..."`.
3. Read skill profiles before applying one to a piece of work: `k2so skills list` then `k2so skills profile <name>`. Your harness (Claude Code, Cursor, Tauri Cmd+T) handles the actual session spawn — K2SO no longer owns the spawn lifecycle.
4. Check `k2so reviews` for pending merge reviews and act: `k2so review approve|reject|feedback <branch>`.
5. If the inbox is empty and no reviews are pending, you're done — `k2so checkin --done` (or `k2so done`) and exit.

Keep your session short. This is triage, not implementation.

Run `k2so glossary <term>` if any K2SO-specific term is unclear.
