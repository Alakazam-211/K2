import { daemonCliGet } from '@/lib/daemon-cli'
import { errorMessage, httpStatusOf } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'

/**
 * One agent CLI as `GET /cli/agents/cli-status` reports it. Every field is
 * always present:
 * - missing: `installed: false`, the rest null / false;
 * - installed: `installed: true`, `version` set, `error: null`;
 * - broken: `installed: true`, `version: null`, `error` says why its
 *   `--version` failed (exited non-zero, timed out, couldn't start).
 *   `version: null` with no error is a harness K2 doesn't ask (hermes).
 * `launches` is whether an agent launch runs the file at `path`.
 */
export interface CliStatus {
  program: string
  installed: boolean
  version: string | null
  path: string | null
  launches: boolean
  error: string | null
}

export interface CliStatusBody {
  ok: true
  /** How long the daemon trusts one `--version` answer. */
  cacheSecs: number
  tools: CliStatus[]
}

export type CliStatusResult =
  | { kind: 'ready'; tools: CliStatus[]; byProgram: Map<string, CliStatus> }
  /** An older daemon without the route: it can't say. */
  | { kind: 'unsupported' }
  | { kind: 'error'; message: string }

function isCliStatus(v: unknown): v is CliStatus {
  if (v === null || typeof v !== 'object') return false
  const t = v as Record<string, unknown>
  const strOrNull = (x: unknown): boolean => x === null || typeof x === 'string'
  return (
    typeof t.program === 'string' &&
    typeof t.installed === 'boolean' &&
    typeof t.launches === 'boolean' &&
    strOrNull(t.version) &&
    strOrNull(t.path) &&
    strOrNull(t.error)
  )
}

/**
 * Which agent CLIs `scope`'s server has. Never throws: a server without
 * the route (404: `route not found` / `route_unclassified`) is
 * `unsupported`; any other failure or an unexpected body is `error`.
 * Every harness K2 can launch (its built-in agent presets) is reported,
 * in launch-bar order; a program that isn't in `byProgram` is one K2
 * can't launch. A harness with no version flag (hermes) comes back
 * installed with `version: null` and no error.
 */
export async function fetchCliStatus(scope: ServerScope): Promise<CliStatusResult> {
  let body: unknown
  try {
    body = await daemonCliGet<CliStatusBody>(scope, 'agents/cli-status')
  } catch (err) {
    if (httpStatusOf(err) === 404) return { kind: 'unsupported' }
    return { kind: 'error', message: errorMessage(err) || 'Couldn’t check which tools are installed' }
  }
  const tools = (body as { tools?: unknown } | null)?.tools
  if (!Array.isArray(tools) || !tools.every(isCliStatus)) {
    return { kind: 'error', message: 'cli-status returned an unexpected body' }
  }
  return { kind: 'ready', tools, byProgram: new Map(tools.map((t) => [t.program, t])) }
}

/** One short line for a reported CLI: "Installed · 2.0.14", "Not installed", … */
export function cliStatusLabel(s: CliStatus): string {
  if (!s.installed) return 'Not installed'
  if (s.error) return 'Installed, not working'
  return s.version ? `Installed · ${s.version}` : 'Installed'
}
