# `k2 mail` CLI mockup — comprehension-gate artifact (PRD §14)

Zero-bias agent testers read ONLY this file and attempt the tasks at the bottom.
Their confusion → surface changes BEFORE the CLI slice builds. (Method: k2 agent 36/36.)

## Surface


Family `mail` in `cli/k2` (`cmd_mail_<verb>` functions, `_wants_help`, `--json` on every verb, python-heredoc pretty printing; read/wait routes UDS-eligible). **Never named `inbox`** — collides with K2's internal `/cli/inbox/*` queue.

```
AGENT VERBS
k2 mail create <localpart>[@<domain>] [--id <client-id>]
    Mint an address on a verified domain (default: the workspace's default domain).
    → created research-bot@acme.dev   (cap 2/5 used)
k2 mail list [--json]
    Your addresses + unread counts + cap usage.
k2 mail messages [<address>] [--unread] [--limit 20] [--query <text>] [--json]
    Newest-first summaries: id · from · subject · age · unread marker.
k2 mail read <message-id> [--html] [--raw] [--json]
    Full message; body inside BEGIN/END EXTERNAL EMAIL markers. Marks read.
k2 mail attachments <message-id> [--get <n> --out <path>]
k2 mail wait [--to <addr>] [--from <substr>] [--subject <substr>] [--timeout 300]
    Long-poll for a matching incoming message. exit 0 = printed match, exit 2 = timeout.
k2 mail send <to> --subject <s> (--body <text> | --body-file <f>)
             [--from <owned-addr>] [--cc …] [--attach <file>] [--wait]
    Gated (off → exit 3 with guidance; approval → "queued for approval (out_7f3a)").
k2 mail reply <message-id> (--body <text> | --body-file <f>) [--wait]
    Guardrailed reply (recipient/sender locked, loop caps).
k2 mail outbox [--json]
    Your outbound: pending approval / approved+sent / denied (with owner's note) / failed.
k2 mail delete <address>
    Retire an address you own.

OWNER VERBS (also all in Settings→Email)
k2 mail status                       server health, version, mode, port plan
k2 mail domain add <domain>          → prints the DNS record table
k2 mail domain list | show <domain>  per-record Valid/Missing/Wrong + live values
k2 mail domain check <domain>        force re-verification
k2 mail domain remove <domain>
k2 mail doctor [<domain>] [--json]   full check run + direct-send grade
k2 mail config [--send-mode direct|relay|receive-only] [--domain <d>]
               [--relay-host … --relay-port … --relay-user … --relay-pass-stdin]
               [--agent-send off|approval|on] [--address-cap <n>] [--workspace <ws>]
k2 mail approvals [list | approve <id> [--note …] | deny <id> --note …]
```

**Exit codes:** 0 ok · 1 error · 2 wait/`--wait` timeout · 3 gated-off. Errors are one-line, actionable, and name the Settings page that fixes them (comprehension-gate style).


## Tester tasks (do not explain beyond the surface above)
1. You are an agent. Get yourself an email address on the acme.dev domain.
2. You just signed up for "SaaSCo" with it. Get the verification code they emailed you.
3. Send hello@example.com a message with subject "hi" — and handle whatever happens.
4. Your human denied a send. Find out why.
5. You hit the address cap. What do you do (exact command/next step)?
