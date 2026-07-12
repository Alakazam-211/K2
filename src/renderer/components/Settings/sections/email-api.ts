// Settings → Email (K2 Mail S7) — typed renderer bindings for the
// daemon's `/cli/mail/*` routes (prd-email-server-v1 §13), plus the
// static SAMPLE fixture the page renders from when the daemon reports
// `supported: false` (pre-mortem #15: the Mac example page must be
// network-silent beyond the one capability read — it never calls the
// live routes and never errors-spams).
//
// Wire shapes are read straight from the daemon source (mail_routes.rs
// + mail/routes_*.rs + mail/domains.rs + mail/addresses.rs +
// mail/send.rs) — camelCase bodies, errors as
// `{"ok":false,"error":{"code","hint"}}` with stable codes. Routes not
// built yet (config get/set = S5-config, doctor = S6) answer a
// structured 501 `not_built`; callers detect it via `isNotBuilt` and
// render the graceful "not available yet" state instead of an error.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

// ── Wire types ───────────────────────────────────────────────────────────

/** GET /cli/mail/status — the capability-gating seam (pre-mortem #15). */
export interface MailStatus {
  ok: boolean
  /** From the DAEMON (Linux = true) — NEVER navigator.platform. */
  supported: boolean
  /** `not-installed` | `installing` | `running` | `degraded` | `stopped` | `disabled` | `error` | … */
  state: string
  version: string | null
  pinnedVersion: string
  hostname: string | null
  portPlan: string | null
  /** S1 persisted-step machine: `{ steps: { <id>: { at } }, current? }`. */
  enableProgress: { steps?: Record<string, { at: number }>; current?: string } | null
  lastError: string | null
  health: unknown
}

/** The ordered enable-step ids — mirrors `supervisor::ENABLE_STEPS`
 *  (the frozen contract between the machine and status renderers). */
export const ENABLE_STEPS: { id: string; label: string }[] = [
  { id: 'preflight', label: 'Preflight checks' },
  { id: 'download', label: 'Download Stalwart (pinned release)' },
  { id: 'verify', label: 'Verify checksum' },
  { id: 'extract', label: 'Install binary' },
  { id: 'system-user', label: 'Create system user' },
  { id: 'dirs', label: 'Create data directories' },
  { id: 'config', label: 'Clear stale configuration' },
  { id: 'unit', label: 'Install systemd unit' },
  { id: 'start', label: 'First start (bootstrap mode)' },
  { id: 'bootstrap', label: 'Complete guided setup' },
  { id: 'restart-normal', label: 'Restart into normal mode' },
  { id: 'server-config', label: 'Configure listeners' },
  { id: 'service-account', label: 'Create service account' },
  { id: 'api-key', label: 'Mint scoped API key' },
  { id: 'recovery-off', label: 'Remove recovery credential' },
  { id: 'restart', label: 'Restart into final config' },
]

export interface PreflightCheck {
  id: string
  label: string
  status: 'pass' | 'warn' | 'fail' | 'info' | 'skipped'
  detail: string
}

export interface PreflightReport {
  ok: boolean
  portPlan: string | null
  checks: PreflightCheck[]
}

/** One row of a domain's DNS record table (§6.2). */
export interface DnsRecordRow {
  /** `mx` | `spf` | `dkim:<selector>` | `dmarc` | `ptr` | `adv:<n>`. */
  id: string
  category: 'required' | 'instruction' | 'advanced'
  type: string
  name: string
  purpose: string
  /** The COPY value (TXT chunks joined). */
  expected: string
  /** Zone-file form (long TXT keeps its quoted split). */
  expectedDisplay: string
  chunks?: string[]
  status: 'pending' | 'valid' | 'missing' | 'wrong' | 'unknown' | 'unverifiable' | 'optional'
  /** What DNS actually serves — populated on Wrong (the diff) and,
   *  for context, on Missing when unrelated records exist. */
  live?: string[] | null
  checkedAt?: number | null
}

/** Owner `domain/list` row. */
export interface DomainSummary {
  domain: string
  status: string
  sendMode: 'direct' | 'relay' | 'receive-only' | string
  verifiedAt: number | null
  lastCheckedAt: number | null
  dmarcNag: boolean
  records: { valid: number; missing: number; wrong: number; pending: number; unknown: number }
}

/** `domain/show` (also what add/check return). */
export interface DomainDetail {
  ok: boolean
  domain: string
  status: string
  sendMode: string
  verifiedAt: number | null
  lastCheckedAt: number | null
  createdAt: number
  dmarcNag: boolean
  records: DnsRecordRow[]
  zoneFile: string
  note: string
}

/** Owner `address/list?all=true` row (retired included). */
export interface AddressRow {
  id: string
  address: string
  status: 'active' | 'retired' | string
  createdAt: number
  retiredAt: number | null
  holderProjectId: string
  holderWorkspace: string | null
}

/** One outbound row (`outbox` + the approvals queue share the shape). */
export interface OutboundItem {
  id: string
  from: string
  to: string[]
  cc: string[]
  subject: string
  status: 'pending_approval' | 'approved' | 'rejected' | 'submitted' | 'failed' | string
  statusNote: string
  note: string | null
  agentName: string | null
  decidedBy: string | null
  createdAt: number
  decidedAt: number | null
  sentAt: number | null
}

/** Approvals-queue item — outbound + the owner-view extras. The body
 *  rides ONLY as this bounded plain-text preview (≤280 chars); it is
 *  never HTML and never auto-rendered (§8.4 / pre-mortem: bodies are
 *  untrusted external content). */
export interface ApprovalItem extends OutboundItem {
  bodyPreview: string | null
  workspace: string | null
  expiresAt: number
}

// ── Error helpers ────────────────────────────────────────────────────────

/** Extract the daemon's structured `{code, hint}` from a thrown error
 *  (daemon-cli surfaces non-2xx bodies as the Error message; mail
 *  routes answer `{"ok":false,"error":{"code","hint"}}`). */
export function mailErrorInfo(err: unknown): { code?: string; hint?: string } {
  const msg = err instanceof Error ? err.message : String(err)
  try {
    const parsed = JSON.parse(msg)
    const e = parsed?.error
    if (e && typeof e === 'object') {
      return {
        code: typeof e.code === 'string' ? e.code : undefined,
        hint: typeof e.hint === 'string' ? e.hint : undefined,
      }
    }
  } catch {
    /* not the JSON error shape — fall through */
  }
  return {}
}

/** User-facing message: the daemon's hint verbatim when present. */
export function mailErrorMessage(err: unknown): string {
  const { hint } = mailErrorInfo(err)
  if (hint) return hint
  return err instanceof Error ? err.message : String(err)
}

/** A structured 501 `not_built` — the route's slice hasn't landed
 *  (config = S5-config, doctor = S6). Renders as "not available yet",
 *  never as an error. */
export function isNotBuilt(err: unknown): boolean {
  return mailErrorInfo(err).code === 'not_built'
}

// ── API calls (live daemon only — never called in sample mode) ──────────

export async function fetchMailStatus(): Promise<MailStatus> {
  return daemonCliGet<MailStatus>('mail/status')
}

export async function fetchPreflight(): Promise<PreflightReport> {
  const res = await daemonCliGet<{ ok: boolean; report: PreflightReport }>('mail/preflight')
  return res.report
}

/** POST server/enable. Success is EITHER `{state:'installing'}` (poll
 *  status for enableProgress) or `{alreadyEnabled:true}`; a failing
 *  preflight comes back 200 with `ok:false` + the report. */
export interface EnableResponse {
  ok: boolean
  state?: string
  alreadyEnabled?: boolean
  hint?: string
  preflight?: PreflightReport
  error?: { code: string; hint: string }
  report?: PreflightReport
}

export async function enableServer(hostname: string): Promise<EnableResponse> {
  return daemonCliPost<EnableResponse>('mail/server/enable', { hostname })
}

export async function disableServer(): Promise<{ ok: boolean; warning?: string }> {
  return daemonCliPost('mail/server/disable', {})
}

export async function uninstallServer(
  purgeData: boolean,
  confirmHostname?: string,
): Promise<{ ok: boolean; purged: boolean }> {
  return daemonCliPost('mail/server/uninstall', { purgeData, confirmHostname })
}

export async function fetchDomains(): Promise<DomainSummary[]> {
  const res = await daemonCliGet<{ ok: boolean; domains: DomainSummary[] }>('mail/domain/list')
  return Array.isArray(res?.domains) ? res.domains : []
}

export async function fetchDomainDetail(domain: string): Promise<DomainDetail> {
  return daemonCliGet<DomainDetail>('mail/domain/show', { domain })
}

export async function addDomain(domain: string): Promise<DomainDetail> {
  return daemonCliPost<DomainDetail>('mail/domain/add', { domain })
}

export async function checkDomainNow(domain: string): Promise<DomainDetail> {
  return daemonCliPost<DomainDetail>('mail/domain/check', { domain })
}

export async function removeDomain(
  domain: string,
  purge: boolean,
): Promise<{ ok: boolean; retiredAddresses: number }> {
  return daemonCliPost('mail/domain/remove', { domain, confirm: true, purge })
}

export async function fetchAllAddresses(): Promise<AddressRow[]> {
  const res = await daemonCliGet<{ ok: boolean; addresses: AddressRow[] }>('mail/address/list', {
    all: true,
  })
  return Array.isArray(res?.addresses) ? res.addresses : []
}

/** Retire an address. `project` = the HOLDER workspace (id/name/path —
 *  the daemon resolves); the route enforces workspace ownership. */
export async function retireAddress(project: string, address: string): Promise<void> {
  await daemonCliPost('mail/address/delete', { project, address })
}

export async function fetchApprovals(): Promise<ApprovalItem[]> {
  const res = await daemonCliGet<{ ok: boolean; pending: ApprovalItem[] }>('mail/approvals/list')
  return Array.isArray(res?.pending) ? res.pending : []
}

export async function approveOutbound(
  id: string,
  note?: string,
): Promise<{ ok: boolean; status: string }> {
  return daemonCliPost('mail/approvals/approve', note ? { id, note } : { id })
}

export async function denyOutbound(id: string, note: string): Promise<{ ok: boolean }> {
  return daemonCliPost('mail/approvals/deny', { id, note })
}

/** Per-workspace outbox (the decided history view — there is no
 *  owner-wide history route yet; the Approvals panel offers a
 *  workspace picker over this). */
export async function fetchOutbox(project: string): Promise<OutboundItem[]> {
  const res = await daemonCliGet<{ ok: boolean; outbox: OutboundItem[] }>('mail/outbox', {
    project,
    limit: 50,
  })
  return Array.isArray(res?.outbox) ? res.outbox : []
}

/** GET /cli/mail/config — still 501 `not_built` while the S5-config
 *  sub-slice is under construction; callers branch on `isNotBuilt`. */
export async function fetchMailConfig(): Promise<Record<string, unknown>> {
  return daemonCliGet('mail/config')
}

/** POST /cli/mail/config/set (send-mode / relay / caps / gating) —
 *  same 501 story as the GET. Body shape per PRD §11 `k2 mail config`. */
export async function setMailConfig(body: Record<string, unknown>): Promise<void> {
  await daemonCliPost('mail/config/set', body)
}

/** GET /cli/mail/doctor — 501 until S6. */
export async function fetchDoctor(): Promise<Record<string, unknown>> {
  return daemonCliGet('mail/doctor')
}

// ── Unified inbox access (GH #28 — one permission layer, hosted OR
//    linked) ───────────────────────────────────────────────────────────
//
// Every inbox — whether HOSTED on your own K2 Mail server or LINKED from
// an external IMAP account — is one row in a single catalog with the SAME
// access model: a **Primary** workspace (manages the inbox and holds its
// own level) plus zero-or-more **grants** (other workspaces). A level is
// one of `read` (read only), `draft` (read + save reply drafts), or `send`
// (read + draft + send). **`send` is selectable whenever the inbox reports
// `maxLevel === 'send'`** — hosted inboxes always, and (as of the linked-
// send contract) linked inboxes too. A linked inbox sends out as your real
// external account, ungated for now. Provisioning stays source-
// specific (hosted = an agent mints on a verified domain; linked =
// mail/link/add), but access is unified: grant / revoke / set-level /
// set-primary (transfer) all run through /cli/mail/access/*. Access is an
// IMAP/ownership concern — NOT gated on `status.supported`, so it works
// identically on Mac and Linux. Primary-gating (only the Primary manages)
// is enforced server-side (403 `not-primary`). Rows NEVER carry
// credentials, secret refs, or the username.

/** The access level a workspace holds on an inbox. `read` = read only;
 *  `draft` = read + save reply drafts; `send` = read + draft + send
 *  (offered whenever the inbox's `maxLevel === 'send'` — hosted always,
 *  linked too as of the linked-send contract). */
export type InboxLevel = 'read' | 'draft' | 'send'

/** How an inbox came into existence — `hosted` on your own mail server or
 *  `linked` from an external IMAP account. Both can be send-capable (see
 *  `maxLevel`); a linked inbox sends out as your real external account. */
export type InboxSource = 'hosted' | 'linked'

/** One participant on an inbox — the Primary or a grant. `canManage` lets the
 *  workspace move/organize mail (folders); `canDelete` lets it move messages to
 *  Trash. **`canDelete` requires `canManage`** — the daemon validates this and
 *  the UI must never send `canDelete: true` with `canManage: false`. */
export interface InboxParticipant {
  projectId: string
  workspace: string | null
  level: InboxLevel
  /** May move/organize mail into folders. */
  canManage: boolean
  /** May move messages to Trash (recoverable). Requires `canManage`. */
  canDelete: boolean
}

/** One inbox in the unified catalog (GET /cli/mail/inboxes). NEVER carries
 *  credentials, secret refs, or the username. `primary` + `grants` describe
 *  who may use it; `yourLevel` is the calling principal's effective level
 *  (null if none); `maxLevel` is the highest level this inbox can grant
 *  (`send` for hosted and, as of the linked-send contract, linked too). */
export interface Inbox {
  address: string
  source: InboxSource
  displayName: string | null
  /** `connected` | `running` | `error` | … — health of the inbox. */
  status: string
  /** The one workspace that manages this inbox (holds its own level). */
  primary: InboxParticipant
  /** Other workspaces granted access. */
  grants: InboxParticipant[]
  /** The calling principal's effective level, or null if none. */
  yourLevel: InboxLevel | null
  /** Highest grantable level — `send` for hosted and (as of the linked-send
   *  contract) linked too; `draft` only where send is unavailable. */
  maxLevel: 'draft' | 'send'
  /** Hosted only — the domain the address lives on. */
  domain?: string | null
  /** Linked only — the IMAP host. */
  host?: string | null
  /** Linked only — the TLS mode. */
  tls?: string | null
}

/** GET /cli/mail/inboxes — the unified catalog (hosted + linked). Access
 *  is cross-platform; no `supported` gate. */
export async function fetchInboxes(): Promise<Inbox[]> {
  const res = await daemonCliGet<{ ok: boolean; count: number; inboxes: Inbox[] }>('mail/inboxes')
  return Array.isArray(res?.inboxes) ? res.inboxes : []
}

/** POST /cli/mail/access/grant — the Primary grants a workspace access, or
 *  changes an existing grant's level. `project` = target workspace (name |
 *  path | UUID — the daemon resolves). 403 `not-primary` if the caller
 *  isn't the Primary. `send` is allowed on both hosted and linked inboxes
 *  (subject to the inbox's `maxLevel`). */
export async function grantInboxAccess(body: {
  address: string
  project: string
  level: InboxLevel
}): Promise<{ ok: boolean }> {
  return daemonCliPost('mail/access/grant', body)
}

/** POST /cli/mail/access/revoke — the Primary removes a workspace's grant.
 *  `not_found` if the workspace held no grant; 403 if not the Primary. */
export async function revokeInboxAccess(body: {
  address: string
  project: string
}): Promise<{ ok: boolean }> {
  return daemonCliPost('mail/access/revoke', body)
}

/** POST /cli/mail/access/set-primary — transfer Primary to another
 *  workspace (the old Primary demotes to a grant, keeping its level). 403
 *  `not-primary` unless the caller is the current Primary. */
export async function setInboxPrimary(body: {
  address: string
  project: string
}): Promise<{ ok: boolean }> {
  return daemonCliPost('mail/access/set-primary', body)
}

/** POST /cli/mail/access/set-level — change a workspace's level. Pass the
 *  Primary's own project to change the Primary's level. `send` is allowed on
 *  both hosted and linked inboxes (subject to the inbox's `maxLevel`). */
export async function setInboxLevel(body: {
  address: string
  project: string
  level: InboxLevel
}): Promise<{ ok: boolean }> {
  return daemonCliPost('mail/access/set-level', body)
}

/** POST /cli/mail/access/set-manage — set a workspace's mailbox-management
 *  capabilities. Pass the Primary's own project to change the Primary. Sets
 *  BOTH flags at once; **`canDelete: true` requires `canManage: true`** (the
 *  daemon rejects the invalid combo, and callers must never send it). 403
 *  `not-primary` unless the caller is the current Primary. */
export async function setInboxManage(body: {
  address: string
  project: string
  canManage: boolean
  canDelete: boolean
}): Promise<{ ok: boolean }> {
  return daemonCliPost('mail/access/set-manage', body)
}

// ── Linked-inbox provisioning (Email Link — cross-platform, GH #28) ─────
//
// Connecting an external IMAP account (Gmail app-password / Fastmail /
// IMAP). The app-password is WRITE-ONLY: sent ONCE in the add body,
// vaulted server-side, never returned or listed. After add, the inbox
// appears in the unified catalog (fetchInboxes, `source === 'linked'`) and
// is managed through the shared access layer above. This is an IMAP CLIENT
// — nothing is installed or hosted — so these routes are NOT gated on
// `status.supported`; they work identically on Mac and Linux.

/** POST /cli/mail/link/add body (camelCase). `project` binds the inbox to
 *  its initial PRIMARY workspace (name | path | UUID — the daemon
 *  resolves). `password` is the app-password, sent ONCE and vaulted
 *  server-side; it is never returned, listed, or stored client-side. */
export interface LinkInboxBody {
  project: string
  address: string
  host: string
  port?: number
  /** `'implicit-tls'` (993, default) | `'starttls'` (143). */
  tls?: string
  username?: string
  displayName?: string
  /** Blank = the daemon autodetects the Drafts folder at add time. */
  draftsFolder?: string
  /** The app-password — WRITE-ONLY. Never echoed back. */
  password: string
}

/** POST /cli/mail/link/add — live-connects to verify at add time; a bad
 *  drafts folder / login / host surfaces as a structured `{code, hint}`
 *  error (use `mailErrorMessage`). The daemon vaults the password and
 *  returns NO credential. */
export async function addLinkedInbox(
  body: LinkInboxBody,
): Promise<{ ok: boolean; address: string; workspace?: string; draftsFolder?: string | null; hint?: string }> {
  return daemonCliPost('mail/link/add', body)
}

/** POST /cli/mail/link/remove — deletes the linked inbox AND its vault
 *  credential. Identified by `address` (the connected account). */
export async function removeLinkedInbox(
  address: string,
): Promise<{ ok: boolean; address: string; removed: boolean }> {
  return daemonCliPost('mail/link/remove', { address })
}

// ── OAuth-linked inboxes (O4 — Gmail / Microsoft, no app-password) ──────
//
// Provider-owned OAuth linking. Instead of a typed app-password, the daemon
// runs the provider's device/loopback consent flow and vaults the resulting
// refresh token server-side — the UI NEVER sees an access/refresh token or
// an auth code, only the human-facing `userCode` / `verificationUrl` and the
// terminal `state`. Two shapes come back from `start` (discriminated on
// `flow`):
//   • Microsoft → DEVICE flow: show the code + URL, the user approves in any
//     browser, we poll `status`.
//   • Gmail → LOOPBACK flow: the daemon opened the SYSTEM browser server-side
//     (so this only works when the daemon is local). We poll `status`.
// A Gmail link attempted against a REMOTE/headless daemon can't open a
// browser and comes back HTTP 409 `remote_unsupported` — a thrown error the
// caller special-cases via `mailErrorInfo` (a teaching case, not a crash).

/** POST /cli/mail/link/oauth/start result — discriminated on `flow`. Codes/
 *  tokens are NEVER present beyond `userCode`/`verificationUrl`. */
export type OauthStartResult =
  | {
      /** Microsoft device flow: show `userCode` at `verificationUrl`. */
      flow: 'device'
      linkId: string
      userCode: string
      verificationUrl: string
      /** Seconds until the device code expires (poll bound). */
      expiresIn: number
    }
  | {
      /** Gmail loopback flow: the daemon opened the local system browser. */
      flow: 'loopback'
      linkId: string
      hint?: string
    }

/** POST /cli/mail/link/oauth/start — kicks off the provider consent flow.
 *  Owner/Primary-gated (403 if not permitted). Gmail on a remote/headless
 *  daemon throws HTTP 409 `remote_unsupported` (inspect via `mailErrorInfo`;
 *  render the teaching message, not a poll card). No token is ever returned. */
export async function linkOauthStart(args: {
  address: string
  provider: 'gmail' | 'microsoft'
  workspace: string
}): Promise<OauthStartResult> {
  return daemonCliPost('mail/link/oauth/start', args)
}

/** GET /cli/mail/link/oauth/status?linkId=… — long-poll the consent flow.
 *  Owner-gated. `connected` carries the discovered `address`; failure states
 *  may carry a `hint`. Never returns tokens or codes. */
export async function linkOauthStatus(
  linkId: string,
): Promise<{ state: 'pending' | 'connected' | 'denied' | 'expired' | 'error'; address?: string; hint?: string }> {
  return daemonCliGet('mail/link/oauth/status', { linkId })
}

// ── Sample fixture (unsupported/Mac example mode — pre-mortem #15) ──────
//
// Rendered VERBATIM when `supported: false`: the page shows a real-
// looking, fully-populated example with every mutating control
// disabled under the D3 banner. Values mirror the daemon's own test
// fixtures (ZONE_FIXTURE in mail/domains.rs) so the example never
// drifts into shapes the live page couldn't produce.

// The sample server is NOT-INSTALLED so the example page shows the §5
// activation surface (explainer + preflight checklist + the disabled
// [Enable Email Server] button under the D3 banner); the domain /
// address / approvals panels carry their own populated examples.
export const SAMPLE_STATUS: MailStatus = {
  ok: true,
  supported: false,
  state: 'not-installed',
  version: null,
  pinnedVersion: '0.16.10',
  hostname: null,
  portPlan: null,
  enableProgress: null,
  lastError: null,
  health: null,
}

export const SAMPLE_PREFLIGHT: PreflightReport = {
  ok: true,
  portPlan: 'tls-alpn',
  checks: [
    { id: 'os', label: 'Operating system is Linux', status: 'pass', detail: 'Linux (example)' },
    { id: 'mta', label: 'No existing mail server on :25', status: 'pass', detail: 'port 25 is free' },
    { id: 'ports', label: 'SMTP ports 25/465/587 bindable', status: 'pass', detail: 'all free' },
    { id: 'port-443', label: 'Port 443 availability', status: 'pass', detail: ':443 free — TLS-ALPN plan' },
    { id: 'public-ip', label: 'Public IP + reverse DNS', status: 'pass', detail: '203.0.113.7 → mail.acme.dev' },
    { id: 'outbound-25', label: 'Outbound port 25 reachable', status: 'warn', detail: 'blocked by provider — relay mode recommended' },
    { id: 'disk', label: 'Disk ≥ 2 GB free', status: 'pass', detail: '38 GB free' },
    { id: 'ram', label: 'RAM ≥ 1 GB', status: 'pass', detail: '4 GB total' },
  ],
}

const SAMPLE_RECORDS: DnsRecordRow[] = [
  {
    id: 'mx', category: 'required', type: 'MX', name: 'acme.dev',
    purpose: 'Mail routing (MX)',
    expected: '10 mail.acme.dev.', expectedDisplay: '10 mail.acme.dev.',
    status: 'valid', live: null, checkedAt: 1751800000,
  },
  {
    id: 'spf', category: 'required', type: 'TXT', name: 'acme.dev',
    purpose: 'SPF (sender policy)',
    expected: 'v=spf1 mx -all', expectedDisplay: '"v=spf1 mx -all"',
    status: 'wrong', live: ['v=spf1 include:old-provider.example ~all'], checkedAt: 1751800000,
  },
  {
    id: 'dkim:202601e', category: 'required', type: 'TXT', name: '202601e._domainkey.acme.dev',
    purpose: 'DKIM signing key (Ed25519)',
    expected: 'v=DKIM1; k=ed25519; p=Fo0barEd25519KeyMaterialAAAA=',
    expectedDisplay: '"v=DKIM1; k=ed25519; p=Fo0barEd25519KeyMaterialAAAA="',
    status: 'valid', live: null, checkedAt: 1751800000,
  },
  {
    id: 'dkim:202601r', category: 'required', type: 'TXT', name: '202601r._domainkey.acme.dev',
    purpose: 'DKIM signing key (RSA)',
    expected: 'v=DKIM1; k=rsa; p=MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA0FirstChunk…IDAQAB',
    expectedDisplay: '"v=DKIM1; k=rsa; p=MIIBIjANBgkqhkiG…" "…IDAQAB"',
    chunks: ['v=DKIM1; k=rsa; p=MIIBIjANBgkqhkiG…', '…IDAQAB'],
    status: 'missing', live: null, checkedAt: 1751800000,
  },
  {
    id: 'dmarc', category: 'required', type: 'TXT', name: '_dmarc.acme.dev',
    purpose: 'DMARC policy',
    expected: 'v=DMARC1; p=none; rua=mailto:postmaster@acme.dev',
    expectedDisplay: '"v=DMARC1; p=none; rua=mailto:postmaster@acme.dev"',
    status: 'pending', live: null, checkedAt: null,
  },
  {
    id: 'ptr', category: 'instruction', type: 'PTR', name: '203.0.113.7',
    purpose: 'Reverse DNS (set at your VPS provider)',
    expected: 'Set reverse DNS of 203.0.113.7 → mail.acme.dev at your provider',
    expectedDisplay: 'Set reverse DNS of 203.0.113.7 → mail.acme.dev at your provider',
    status: 'unverifiable', live: null, checkedAt: null,
  },
  {
    id: 'adv:1', category: 'advanced', type: 'CNAME', name: 'autoconfig.acme.dev',
    purpose: 'Mail client autoconfig (optional)',
    expected: 'mail.acme.dev.', expectedDisplay: 'mail.acme.dev.',
    status: 'optional', live: null, checkedAt: null,
  },
  {
    id: 'adv:2', category: 'advanced', type: 'TXT', name: '_mta-sts.acme.dev',
    purpose: 'MTA-STS (optional)',
    expected: 'v=STSv1; id=1719000000', expectedDisplay: '"v=STSv1; id=1719000000"',
    status: 'optional', live: null, checkedAt: null,
  },
]

export const SAMPLE_DOMAINS: DomainSummary[] = [
  {
    domain: 'acme.dev', status: 'pending', sendMode: 'receive-only',
    verifiedAt: null, lastCheckedAt: 1751800000, dmarcNag: true,
    records: { valid: 2, missing: 1, wrong: 1, pending: 1, unknown: 0 },
  },
  {
    domain: 'example.org', status: 'verified', sendMode: 'relay',
    verifiedAt: 1751500000, lastCheckedAt: 1751800000, dmarcNag: false,
    records: { valid: 5, missing: 0, wrong: 0, pending: 0, unknown: 0 },
  },
]

export const SAMPLE_DOMAIN_DETAILS: Record<string, DomainDetail> = {
  'acme.dev': {
    ok: true, domain: 'acme.dev', status: 'pending', sendMode: 'receive-only',
    verifiedAt: null, lastCheckedAt: 1751800000, createdAt: 1751400000,
    dmarcNag: true, records: SAMPLE_RECORDS,
    zoneFile:
      '; Example zone for acme.dev\nacme.dev.\t3600\tIN\tMX\t10 mail.acme.dev.\nacme.dev.\t3600\tIN\tTXT\t"v=spf1 mx -all"\n',
    note: 'records can take up to 48 h to propagate',
  },
  'example.org': {
    ok: true, domain: 'example.org', status: 'verified', sendMode: 'relay',
    verifiedAt: 1751500000, lastCheckedAt: 1751800000, createdAt: 1751300000,
    dmarcNag: false,
    records: SAMPLE_RECORDS.map((r) => ({
      ...r,
      name: r.name.replace('acme.dev', 'example.org'),
      status: r.category === 'required' ? 'valid' : r.status,
      live: null,
    })),
    zoneFile:
      '; Example zone for example.org\nexample.org.\t3600\tIN\tMX\t10 mail.acme.dev.\n',
    note: 'records can take up to 48 h to propagate',
  },
}

export const SAMPLE_ADDRESSES: AddressRow[] = [
  {
    id: 'addr-1', address: 'research-bot@example.org', status: 'active',
    createdAt: 1751510000, retiredAt: null, holderProjectId: 'example-ws-1',
    holderWorkspace: 'research-bot',
  },
  {
    id: 'addr-2', address: 'signup-runner@example.org', status: 'active',
    createdAt: 1751520000, retiredAt: null, holderProjectId: 'example-ws-2',
    holderWorkspace: 'signup-runner',
  },
  {
    id: 'addr-3', address: 'old-crawler@example.org', status: 'retired',
    createdAt: 1751410000, retiredAt: 1751600000, holderProjectId: 'example-ws-2',
    holderWorkspace: 'signup-runner',
  },
]

// Unified-catalog example rows for the hosted addresses above (source:
// 'hosted' → the shared access panel offers Read / Draft / Send). Keyed by
// the same addresses as SAMPLE_ADDRESSES so the Mac example page lines up.
export const SAMPLE_INBOXES: Inbox[] = [
  {
    address: 'research-bot@example.org', source: 'hosted', displayName: null,
    status: 'running',
    primary: { projectId: 'example-ws-1', workspace: 'research-bot', level: 'send', canManage: true, canDelete: true },
    grants: [{ projectId: 'example-ws-2', workspace: 'signup-runner', level: 'read', canManage: false, canDelete: false }],
    yourLevel: 'send', maxLevel: 'send', domain: 'example.org',
  },
  {
    address: 'signup-runner@example.org', source: 'hosted', displayName: null,
    status: 'running',
    primary: { projectId: 'example-ws-2', workspace: 'signup-runner', level: 'draft', canManage: true, canDelete: false },
    grants: [], yourLevel: 'send', maxLevel: 'send', domain: 'example.org',
  },
]

export const SAMPLE_APPROVALS: ApprovalItem[] = [
  {
    id: 'out_7f3a', from: 'research-bot@example.org',
    to: ['support@vendor.example'], cc: [],
    subject: 'Request: API rate-limit increase for research project',
    status: 'pending_approval',
    statusNote: 'queued for approval — your human decides in Settings → Email → Approvals',
    note: null, agentName: 'research-bot', decidedBy: null,
    createdAt: 1751790000, decidedAt: null, sentAt: null,
    bodyPreview:
      'Hello,\n\nWe are evaluating your API for a research project and are hitting the default rate limit. Could you raise the limit for the account registered under research-bot@example.org?\n\nThanks!',
    workspace: 'research-bot', expiresAt: 1752394800,
  },
]
