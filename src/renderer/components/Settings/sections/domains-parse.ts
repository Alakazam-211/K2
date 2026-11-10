// Settings → Custom Domains: every daemon answer the page reads is checked
// here, at the boundary. A missing optional field takes its default; an
// answer of the wrong shape throws `DomainsParseError`, which the page shows
// as a failed read (never a crash, never a silently empty list).
//
//   GET /cli/domains    {ok, domains: DomainRow[]}
//   GET /cli/firewall   {ok, view: {public, box, upstream}, ssh, guard, applies, front}

export class DomainsParseError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'DomainsParseError'
  }
}

type Obj = Record<string, unknown>

function isObj(v: unknown): v is Obj {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}
const str = (v: unknown): string | null => (typeof v === 'string' && v !== '' ? v : null)
const bool = (v: unknown, d = false): boolean => (typeof v === 'boolean' ? v : d)
const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)
const strList = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string' && x !== '') : [])

// ── Domains ─────────────────────────────────────────────────────────

/** A name's certificate as the daemon probed it. `expiresAt` is unix
 *  seconds (an ISO date string is accepted too). */
export type CertState = { state: string; issuer: string | null; expiresAt: number | null }

/** How K2 renews a name's certificate (`renewal`), when it says. */
export type CertRenewal = { state: string; lastError: string | null }

export type DnsRecord = { type: string; name: string; value: string }

/** A custom App name's serving state (daemon `custom_domains::status_json`). */
export type AppNameStatus = {
  app: string
  state: string
  live: boolean
  notServedYet: boolean
  /** Served by your own web server, not by K2. */
  external: boolean
  /** The daemon's own words (kept for reference; the page writes its own). */
  words: string
  primary: boolean
  lastError: string | null
  warning: string | null
  /** `unknown_app`: no App by that name on this server. */
  error: string | null
  txt: DnsRecord | null
  /** K2 Connect routes this name to this server. */
  viaRelay: boolean
  /** The DNS records to set, for the path in `recordsPath`. */
  records: DnsRecord[]
  recordsPath: 'direct' | 'connect' | null
  /** Before K2 Connect accepted a claim: what it is waiting for. */
  txtPending: string | null
  pathB: { available: boolean; why: string | null; meter: string | null; upgrade: string | null } | null
}

export type DomainNameRow = {
  hostname: string
  role: string
  cert: CertState
  renewal: CertRenewal | null
  app: AppNameStatus | null
}

export type DomainRow = {
  apex: string
  zoneId: string | null
  dnsWrite: boolean
  /** `active` | `pending_ns` on k2.dev, or `winddown` / `suspended`; null for your own DNS. */
  status: string | null
  /** Nameservers k2.dev wants at the registrar. */
  nameservers: string[]
  /** k2.dev added the zone to this account on attach. */
  created: boolean
  /** Records serve but can't change (plan on hold at k2.dev). */
  readOnly: boolean
  /** The zone was not in k2.dev's last list for this server. */
  zoneMissing: boolean
  /** The last failed check with k2.dev, if any since the last good one. */
  checkError: string | null
  names: DomainNameRow[]
}

function parseRecord(v: unknown): DnsRecord | null {
  if (!isObj(v)) return null
  const type = str(v.type)
  const name = str(v.name)
  const value = str(v.value)
  return type && name && value ? { type, name, value } : null
}

function parseCert(v: unknown): CertState {
  if (!isObj(v)) return { state: 'missing', issuer: null, expiresAt: null }
  let expiresAt = num(v.expiresAt)
  if (expiresAt === null && typeof v.expiresAt === 'string') {
    const ms = Date.parse(v.expiresAt)
    expiresAt = Number.isFinite(ms) ? Math.floor(ms / 1000) : null
  }
  return { state: str(v.state) ?? 'missing', issuer: str(v.issuer), expiresAt }
}

function parseApp(v: unknown, at: string): AppNameStatus | null {
  if (v === undefined || v === null) return null
  if (!isObj(v)) throw new DomainsParseError(`${at}.app is not an object`)
  const app = str(v.app)
  if (!app) throw new DomainsParseError(`${at}.app has no App name`)
  const pb = isObj(v.pathB) ? v.pathB : null
  const path = v.recordsPath === 'direct' || v.recordsPath === 'connect' ? v.recordsPath : null
  return {
    app,
    state: str(v.state) ?? 'waiting_dns',
    live: bool(v.live),
    notServedYet: bool(v.notServedYet, !bool(v.live)),
    external: bool(v.external),
    words: str(v.words) ?? '',
    primary: bool(v.primary),
    lastError: str(v.lastError),
    warning: str(v.warning),
    error: str(v.error),
    txt: parseRecord(v.txt),
    viaRelay: bool(v.viaRelay),
    records: Array.isArray(v.records) ? v.records.map(parseRecord).filter((r): r is DnsRecord => r !== null) : [],
    recordsPath: path,
    txtPending: str(v.txtPending),
    pathB: pb ? { available: bool(pb.available), why: str(pb.why), meter: str(pb.meter), upgrade: str(pb.upgrade) } : null,
  }
}

function parseName(v: unknown, at: string): DomainNameRow {
  if (!isObj(v)) throw new DomainsParseError(`${at} is not an object`)
  const hostname = str(v.hostname)
  if (!hostname) throw new DomainsParseError(`${at} has no hostname`)
  const r = isObj(v.renewal) ? v.renewal : null
  return {
    hostname,
    role: str(v.role) ?? 'other',
    cert: parseCert(v.cert),
    renewal: r ? { state: str(r.state) ?? 'ok', lastError: str(r.lastError) } : null,
    app: parseApp(v.app, at),
  }
}

function parseDomain(v: unknown, i: number): DomainRow {
  const at = `domains[${i}]`
  if (!isObj(v)) throw new DomainsParseError(`${at} is not an object`)
  const apex = str(v.apex)
  if (!apex) throw new DomainsParseError(`${at} has no domain name`)
  if (v.names !== undefined && v.names !== null && !Array.isArray(v.names)) {
    throw new DomainsParseError(`${apex}: its names are not a list`)
  }
  const names = Array.isArray(v.names) ? v.names.map((n, j) => parseName(n, `${apex} names[${j}]`)) : []
  return {
    apex,
    zoneId: str(v.zoneId),
    dnsWrite: bool(v.dnsWrite),
    status: str(v.status),
    nameservers: strList(v.nameservers),
    created: bool(v.created),
    readOnly: bool(v.readOnly),
    zoneMissing: bool(v.zoneMissing),
    checkError: str(v.checkError),
    names,
  }
}

/** `GET /cli/domains`. Throws `DomainsParseError` when it isn't a list of domains. */
export function parseDomainList(raw: unknown): DomainRow[] {
  if (!isObj(raw)) throw new DomainsParseError('the answer is not an object')
  if (!Array.isArray(raw.domains)) throw new DomainsParseError('the answer has no list of domains')
  return raw.domains.map(parseDomain)
}

/** One domain in a `POST /cli/domains` (attach) or `refresh` answer, or null. */
export function parseDomainAnswer(raw: unknown): DomainRow | null {
  if (!isObj(raw) || !isObj(raw.domain)) return null
  try {
    return parseDomain(raw.domain, 0)
  } catch {
    return null
  }
}

// ── Firewall ────────────────────────────────────────────────────────

export type FirewallPort = {
  port: number
  proto: 'tcp' | 'udp'
  /** `ssh`, `front443`, `front80`, `skin-direct`, `hostmail`, `sew-voice`, `daemon`, `owner`. */
  source: string
  /** Empty = anyone. */
  allowFrom: string[]
  why: string
}

export type FirewallStatus = {
  ports: FirewallPort[]
  behindNat: boolean
  /** `k2`: K2 applies the list at the provider; `owner`: nobody does. */
  managedBy: string
  ssh: { port: number; allowFrom: string[] }
  guard: { enabled: boolean; connsPerIp: number | null; writesPerMin: number | null; refused: number; limited: number } | null
  /** The :443 front and its :80 redirect, when the daemon says. */
  front: { enabled: boolean; httpOn: boolean; httpServing: boolean; relayNames: number } | null
}

function parsePort(v: unknown, i: number): FirewallPort {
  if (!isObj(v)) throw new DomainsParseError(`ports[${i}] is not an object`)
  const port = num(v.port)
  if (port === null) throw new DomainsParseError(`ports[${i}] has no port number`)
  const allow = strList(v.allowFrom).filter((a) => a !== 'any')
  return { port, proto: v.proto === 'udp' ? 'udp' : 'tcp', source: str(v.source) ?? 'owner', allowFrom: allow, why: str(v.why) ?? '' }
}

/** `GET /cli/firewall`. Throws `DomainsParseError` when it has no port list. */
export function parseFirewallStatus(raw: unknown): FirewallStatus {
  if (!isObj(raw)) throw new DomainsParseError('the answer is not an object')
  const view = isObj(raw.view) ? raw.view : null
  if (!view || !Array.isArray(view.public)) throw new DomainsParseError('the answer has no list of open ports')
  const box = isObj(view.box) ? view.box : {}
  const upstream = isObj(view.upstream) ? view.upstream : {}
  const ssh = isObj(raw.ssh) ? raw.ssh : {}
  const guard = isObj(raw.guard) ? raw.guard : null
  const gs = guard && isObj(guard.settings) ? guard.settings : null
  const gc = guard && isObj(guard.counters) ? guard.counters : {}
  const front = isObj(raw.front) ? raw.front : null
  const http = front && isObj(front.http) ? front.http : null
  return {
    ports: view.public.map(parsePort),
    behindNat: bool(box.behindNat),
    managedBy: str(upstream.managedBy) ?? 'owner',
    ssh: { port: num(ssh.port) ?? 22, allowFrom: strList(ssh.allowFrom).filter((a) => a !== 'any') },
    guard: gs
      ? {
          enabled: bool(gs.enabled),
          connsPerIp: num(gs.connsPerIp),
          writesPerMin: num(gs.writesPerMin),
          refused: num(gc.perIpRefused) ?? 0,
          limited: num(gc.guardLimited) ?? 0,
        }
      : null,
    front: front
      ? { enabled: bool(front.enabled), httpOn: bool(http?.on), httpServing: bool(http?.serving), relayNames: num(http?.relayNames) ?? 0 }
      : null,
  }
}
