<!-- DEFAULT TEMPLATE — K2 scaffolded this for the workspace's primary agent in manager mode.
     Edit below to customize what the agent does when the heartbeat wakes it.
     Delete this comment once you've made it your own. -->

# On wake-up — Workspace Primary Agent (manager mode)

1. Run `k2 organization bulletin` for today's bulletin of orgs and heads.
2. Triage your inbox (`k2 inbox`). For each item, decide: act on it yourself, file it for later (`k2 inbox move <id> <folder>`), or — if it should go elsewhere — forward via `k2 msg <workspace> --inbox-wake <path>` / `--inbox-silent <path>` (recipient opens with `k2 inbox read <id>`).
3. Read skill profiles before applying one to a piece of work: `k2 skills list` then `k2 skills profile <name>`. Your harness (Claude Code, Cursor, Tauri Cmd+T) handles the actual session spawn — K2 no longer owns the spawn lifecycle.
4. Check `k2 reviews` for pending merge reviews and act: `k2 review approve|reject|feedback <branch>`.
5. If the inbox is empty and no reviews are pending, you're done: `k2 done` and exit.

Keep your session short. This is triage, not implementation.

Run `k2 glossary <term>` if any K2-specific term is unclear.
