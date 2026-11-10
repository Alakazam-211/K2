/**
 * The Org Chart's route shapes (prd-org-chart-v1 §6.2). Mirrors
 * `crates/k2-core/src/org_chart/wire.rs`, the one source on the daemon
 * side; keep the two in step. camelCase on the wire.
 *
 * Every route lives under `/cli/organization/`. A refusal is always
 * `{ ok: false, error: { code, hint } }` (see `OrgChartError`). While the
 * Org Chart is off every route answers 409 `org_chart_off`.
 *
 * Settings: `orgChart: boolean` and `orgChartNight` (below) on
 * `/cli/settings/update` (Owner or Admin). The per-agent switch "Manage the
 * org chart" is `POST /cli/agent-access/set {toggle: "org_chart_manage",
 * workspace, value}` (the Owner turns it on; the Owner or an Admin turns it
 * off); each chart agent's `canManage` reads it back.
 *
 * Live updates: the app socket's `{ kind: "org_chart_changed", revision }`;
 * reload the chart when `revision` is newer than yours.
 */

/** `orgChartNight` in settings: `HH:MM`, owner's timezone, in order within 22:00–07:00. */
export interface OrgChartNight {
  runAt: string
  lastCallAt: string
  /** The read marker's day start. `editionAt` is accepted on write as this. */
  dayStartsAt: string
}

export interface OrgChartError {
  ok: false
  error: {
    /** e.g. org_chart_off, org_manage_off, people_only, stale_revision, line_top_to_top, line_loop, line_exists, line_self, agent_in_another_org, head_not_member, not_org_head, bulletin_too_long, bulletin_line_too_long, bulletin_stamp, org_name_taken, org_name_invalid, purpose_too_long, ambiguous_name, not_found, role_required, usage */
    code: string
    hint: string
  }
}

export interface OrgAgentRef {
  id: string
  handle: string
}

export interface OrgBulletin {
  lines: string[]
  /** unix seconds */
  updatedAt: number
  /** `owner`, `login:<user>`, `agent:<handle>` */
  updatedBy: string
}

/** `GET /cli/organization/chart` — the whole canvas, laid out by the daemon. */
export interface OrgChart {
  ok: true
  revision: number
  /** An Owner or Admin person (may arrange and manage). */
  canEdit: boolean
  /** The caller may change orgs, members, heads and lines (also an agent with the switch). */
  canManage: boolean
  /** The caller may drag and tidy (Owner/Admin people only; = canEdit). */
  canArrange: boolean
  orgs: OrgChartOrg[]
  /** Every agent on this server, then agents on other servers an agent here is connected to (`server` set). */
  agents: OrgChartAgent[]
  lines: OrgLine[]
  /** The dotted layer: who may message whom (agent ids or `remote:` ids), one row per pair. */
  connections: { a: string; b: string }[]
  /** The daemon's layout, the truth: one row per org and agent. */
  nodes: OrgChartNode[]
  card: { w: number; h: number }
}

/** A member's x/y is relative to its bucket (as `layout/move` takes it); an
 *  org's and a loose agent's are canvas coordinates. w/h: an org's size,
 *  null for a card. */
export interface OrgChartNode {
  kind: 'agent' | 'org'
  id: string
  x: number
  y: number
  w: number | null
  h: number | null
  placedBy: 'auto' | 'person'
}

export interface OrgChartOrg {
  id: string
  name: string
  purpose: string
  headId: string | null
  /** Head first, then by name. */
  memberIds: string[]
  bulletin: OrgBulletin | null
  /** Canvas rectangle of the bucket. */
  x: number
  y: number
  w: number
  h: number
  placedBy: 'auto' | 'person'
}

export interface OrgChartAgent {
  id: string
  /** What the card shows: the handle, or `agent (host)` for another server's. */
  name: string
  handle: string
  /** First line of the agent's Role in its AGENTS.md ('' when none). */
  role: string
  color: string
  iconUrl: string | null
  /** null: this server; else the server the agent lives on. */
  server: string | null
  orgId: string | null
  isHead: boolean
  /** Its "Manage the org chart" switch. */
  canManage: boolean
  /** Card top-left on the canvas (absolute, members included). */
  x: number
  y: number
  placedBy: 'auto' | 'person'
}

export interface OrgLineEnd {
  kind: 'agent' | 'org'
  id: string
  name: string
}

/** `below`: `a` sits above `b` (top to bottom). `beside`: side by side. */
export interface OrgLine {
  id: string
  kind: 'below' | 'beside'
  a: OrgLineEnd
  b: OrgLineEnd
}

export interface OrgMember {
  id: string
  handle: string
  role: string
  isHead: boolean
}

/** One org in list / show / mine / write answers. */
export interface OrgSummary {
  id: string
  name: string
  purpose: string
  head: OrgAgentRef | null
  members: OrgMember[]
  bulletin: OrgBulletin | null
  createdAt: number
  updatedAt: number
  /** `show` only: lines touching the org or its members. */
  lines?: OrgLine[]
}

/** `GET /cli/organization/bulletin` (`peek=1` doesn't mark it read). */
export interface OrgServerBulletin {
  ok: true
  /** `YYYY-MM-DD`, owner's timezone; a day starts at `dayStartsAt`. */
  day: string
  zone: string
  text: string
  orgs: {
    id: string
    name: string
    purpose: string
    head: OrgAgentRef | null
    lines: string[]
    updatedAt: number | null
    updatedToday: boolean
  }[]
  markedRead: boolean
}

/** Write answers carry the new revision. */
export interface OrgWriteOk {
  ok: true
  revision: number
}

/**
 * Gesture bodies (all POST, JSON). Orgs and agents may be named or given by id.
 * - create {name, purpose?} · rename {org, name} · purpose {org, purpose} · delete {org}
 * - member/add {org, agent} · member/move {agent, org} · member/remove {org, agent}
 * - head {org, agent | "none"}
 * - line/add {kind: below|beside, a, b} with a/b as {kind, id} (below: a sits
 *   above b; `under` is the other way round), or {a, b, fromAnchor, toAnchor}
 *   (anchors top|bottom|left|right; top→top is refused `line_top_to_top`)
 * - line/remove {id} or {a, b}
 * - layout/move {revision, kind: 'agent'|'org', id, x, y, w?, h?} (a member's
 *   x/y relative to its bucket; 409 `stale_revision` when the chart moved)
 * - layout/tidy {revision?} (409 `stale_revision` when sent and stale)
 * - bulletin/set {org, lines}
 */
export type OrgAnchor = 'top' | 'bottom' | 'left' | 'right'
