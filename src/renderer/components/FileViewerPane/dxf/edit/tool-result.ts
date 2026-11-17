// One edit verb's outcome as the CAD pack's kit reports it (`mpc.result/1`):
// whether it landed, the versions, the digest a dry run would store, the
// change itself (`data`, `cad.change/1`), or the refusal (`error`: its id,
// message, next step and details). Read defensively: a missing or mistyped
// field is dropped, never guessed.

export type ToolError = {
  id: string
  message: string
  next: string
  details: Record<string, unknown>
}

export type ToolResult = {
  ok: boolean
  exit: number
  verb: string
  dryRun: boolean
  /** The version the call landed on, and the one it made (null: a dry run, a refusal or no change). */
  base: number | null
  after: number | null
  /** What a dry run's bytes hash to: the real call passes it back as `--expect`. */
  afterDigest: string | null
  /** The change (`cad.change/1`), as sent. */
  data: unknown
  error: ToolError | null
  /** The verb's own line: "no change: …" when the value was already that. */
  stdout: string
}

type Obj = Record<string, unknown>
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v)
const str = (v: unknown): string | null => (typeof v === 'string' ? v : null)
const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)

/** The kit's result object itself (`mpc.result/1`), or null when it isn't one. */
export function parseToolResult(r: unknown): ToolResult | null {
  if (!isObj(r) || r.schema !== 'mpc.result/1') return null
  const version = isObj(r.version) ? r.version : {}
  const changed = isObj(r.changed) ? r.changed : {}
  const bytes = isObj(changed.bytes) ? changed.bytes : {}
  return {
    ok: r.ok === true,
    exit: num(r.exit) ?? (r.ok === true ? 0 : 2),
    verb: str(r.verb) ?? '',
    dryRun: r.dry_run === true,
    base: num(version.base),
    after: num(version.after),
    afterDigest: str(bytes.after_digest),
    data: r.data ?? null,
    error: errorOf(r.error),
    stdout: str(r.stdout) ?? '',
  }
}

const errorOf = (v: unknown): ToolError | null =>
  isObj(v) && typeof v.id === 'string' && v.id
    ? { id: v.id, message: str(v.message) ?? '', next: str(v.next) ?? '', details: isObj(v.details) ? v.details : {} }
    : null

/**
 * The doc tool route's reply (`{exit, stdout, stderr, version, result, …}`):
 * the kit's result under `result`, its stdout from the reply when the result
 * has none; a reply with no result but an `error` with an id is that
 * refusal; anything else is null.
 */
export function parseToolReply(reply: unknown): ToolResult | null {
  if (!isObj(reply)) return null
  const r = parseToolResult(reply.result)
  if (r) return r.stdout || typeof reply.stdout !== 'string' ? r : { ...r, stdout: reply.stdout }
  const e = errorOf(reply.error)
  return e ? localRefusal(e.id, e.message, e.details) : null
}

/** A refusal made here (the route missing, the server unreachable), shaped like the kit's. */
export function localRefusal(id: string, message = '', details: Record<string, unknown> = {}): ToolResult {
  return { ok: false, exit: 2, verb: '', dryRun: false, base: null, after: null, afterDigest: null, data: null, error: { id, message, next: '', details }, stdout: '' }
}
