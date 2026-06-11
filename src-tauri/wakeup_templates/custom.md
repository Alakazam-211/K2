<!-- DEFAULT TEMPLATE — K2SO scaffolded this for a custom agent.
     Edit below to customize what this agent does when the heartbeat wakes it.
     Delete this comment once you've made it your own. -->

# On wake-up — Custom Agent

1. Run `k2so checkin` to see your state: peer messages, recent activity, pending reviews, and inbox arrivals.
2. Triage your inbox (`k2so inbox`) — read top-level items (`k2so inbox read <id>`), file them with `k2so inbox move <id> <folder>` (folders are agent-organized, like email).
3. If you have items in your active folder (`k2so inbox list active`), resume the highest-priority one. Pick up where you left off — your prior session transcript is your best context.
4. To reply to a sender: `k2so inbox respond <id> "text"`. To raise something with another workspace: `k2so msg <workspace> "text"` (live) or `k2so msg <workspace> --inbox --title "..." --body "..."` (async).
5. Report status as you work: `k2so checkin --status "..."`. When done, `k2so checkin --done` (or `k2so done`).

Check for messages from peers every wake — live `msg` deliveries arrive in your session immediately; inbox items wait until you read them.

Run `k2so glossary <term>` for any unfamiliar K2SO word (workspace, skill, inbox, heartbeat, etc.).
