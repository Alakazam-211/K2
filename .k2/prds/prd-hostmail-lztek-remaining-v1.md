# Mini-PRD — LZTEK remaining after 0.40.147 e2e

**Date:** 2026-09-18 · **Status:** wrap remaining it-email concerns. Hop the code slices; ops flips stay theirs.  
**Source:** it-email inbox `it-email-remaining-issues-rechecked-sieve-list-closed` + Rosson Thread (unlimited quota).  
**k2-dev-web:** not implementer. Keep cPanel. No `hostmail disable`/`enable`. No EXPUNGE. No MX0 swap.

---

## Closed (do not re-open)

forward unset / ooo / footer / mailing-list query · LE cert on 443/465 · IMAP 993+143 · lztek.io verified · create-on-pending · import+canManage · scratch e2e.a/b · hostmail CLI e2e green · **v0.40.147 LIVE**.

---

## Remaining

| # | Concern | Kind | This hop? |
|---|---|---|---|
| **L1** | New-mint quota 1 GiB / 10k; workspace 50G/500k does not retrofit. `api@` ×2 and `lancelot.reese@` ×1 451 over-quota in queue. | Product + ops | **Code:** mint default **unlimited**. **Ops:** it-email `quota set` those boxes (0 = unlimited). Do not drop the queue. |
| **L2** | `autoconfig show lztek.io` 6/7 records still name **mail.lztek.k2.dev**. 0 name `mail.lztek.io`. Apply is unsafe until the recipe uses the attached hostname. | Bug | **Yes.** Rewrite like `domain show` (C25): attached `role=mail` hostname, else `mail_server.hostname`. Show-only until show is clean. |
| **L3** | Live `_dmarc` `p=reject`; hostmail expected `p=none` (`match=wrong`). `reportAddressUri=mailto:postmaster` not minted. | Recipe vs live DNS | **No rewrite.** `dmarc show` stays honest. `report-to` only if they pick a minted inbox (`dmarc-reports@lztek.io` exists). Plant `rua=` is theirs. |
| **L4** | `sendMode` still **receive-only**; `agentSend` default **off**. | Ops go | **No flip this hop.** Inbound+admin stay theirs. Outbound waits on their `config --send-mode` + doctor. Conservative defaults unchanged. |
| **L5** | DKIM rotate not tried (selectors valid). | Parked | **Still parked.** Do not rotate production selectors. |

---

## Locks

| # | Decision |
|---|---|
| **U1** | Mint fallback `QUOTA_BYTES` / `QUOTA_MAX_MESSAGES` = **0**. Global `AppSettings` defaults = **0**. 0 = unlimited in Stalwart. Mail-manage sets a cap per inbox with `k2 hostmail quota set`. |
| **U2** | Mint constants **do not retrofit**. Existing 451s need `quota set --bytes 0 --messages 0` (or a cap they choose). Disk **and** message count. |
| **U3** | Persisted `settings.json` / workspace `--quota-gb 50` still wins if they set it. Changing the Rust default does not wipe a stored 1 GB. it-email owns workspace config. |
| **U4** | Autoconfig `show`/`apply` pass current mail hostname into `build_rows` + `apply_current_hostname_advanced`. Prefer attached `domain_names.role=mail` on that apex over `mail.<connect>.k2.dev`. Never plant Connect-host CNAMEs/SRV for a custom apex. |
| **U5** | Autoconfig apply remains **show-only until show is clean**. After hop, it-email retests `show`; apply only if every target is `mail.lztek.io` (or the attached mail name). |
| **U6** | Do not rewrite live `_dmarc`. Expected recipe stays `p=none`; live `p=reject` is a **match=wrong** they already accepted. |
| **U7** | Do not `config --send-mode direct` / `--agent-send on`. C12/C27: they may, after doctor. Not this hop. |
| **U8** | DKIM rotate stays parked. |
| **U9** | Extra-gate unchanged. Tests loud. Isolated hop of daemon+CLI. No version bump. No `Co-Authored-By`. |
| **U10** | Keep cPanel. No disable+enable. No EXPUNGE. |

---

## HTTP / CLI

No new routes. Quota 0/0 is the same `GET|POST /cli/mail/quota`. Autoconfig paths unchanged; payload `records[].value` must follow the attached mail hostname.

---

## Tests

- Mint without workspace override stamps `(0, 0)`.
- Workspace override still wins (existing `mint_uses_workspace_quota_override`).
- Autoconfig fixture with `hostname = mail.lztek.io` rewrites `mail.oldhost.k2.dev` CNAME/SRV.
- `k2 mail list` still inboxes.

---

## Out

People hub. MX0 swap. DMARC policy editor. Autoconfig HTTP XML hosting. ngrok yank (Companion drop-later, after 147).
