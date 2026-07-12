# DNS K1 implementation status (Grok → Claude)

**Branch:** `feat/dns-k1` (off `feat/wave0-session-identity`)  
**Worktree:** `~/.grok/worktrees/alakazam-labs-k2/dns-k1`  
**Date:** 2026-07-12  
**Spec:** `~/Desktop/HANDOFF-dns-k1-for-grok.md`

## Commits

| SHA | What |
|-----|------|
| `2256b3f` | Toggles: app `dnsManageEnabled` + workspace column + UI + `dns_manage_allowed_for_path` |
| `1419106` | Daemon: proxy + `/cli/dns/*` principal-bound routes |
| `28c2e82` | CLI: `k2 dns` verbs + exit-3 teaching |

## Verified

- `cargo test -p k2-daemon --lib -- dns` → 28 passed (includes mail dns_verify noise filter)
- `tests/cli/dns_verbs_smoke.sh` → 37 passed
- `tests/cli/id_tool_auth_no_owner_inversion.sh` → 29 passed

## CLI surface

```
k2 dns access [--json]
k2 dns list [--json]
k2 dns records <domain> [--json]
k2 dns record add <domain> <type> <name> <value> [--ttl N] [--priority N]
k2 dns record remove <record-id>
k2 dns verify <domain>
```

Exit **3** when toggle off → Settings → K2 Connect / workspace DNS manage.

## Proxy seam (Claude live verify)

1. Live `~/.k2/tunnel.json` `token` (`k2c_…`)
2. Toggle ON (app and/or workspace)
3. Optional `K2_DNS_API_BASE` (default `https://k2.dev`)
4. `GET /cli/dns/access` then record add against real zone

## Residual for Claude / later

- Grant announce (AGENT.md + channel notice) still TODO on toggle flip
- Live e2e with long.holiday
- K2 `k2 publish --domain` (K2 package) — Claude owns
