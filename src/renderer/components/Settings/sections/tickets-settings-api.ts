// Settings → Tickets (0.45.4): the server's ticket rules, owned by the
// daemon (`settings.json` → `tickets`). Thin client over
// GET/POST /cli/tickets/settings on the window's server: the daemon
// validates, stores, and says whether this login may change them
// (`canEdit`: the owner token or an Owner/Admin login).

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export type TicketRule = 'off' | 'warn' | 'require'

export interface TicketPerson {
  /** Stored value: `owner` or a Connect username. */
  value: string
  label: string
  role: string | null
  disabled: boolean
}

export interface TicketSettings {
  briefPolicy: TicketRule
  assigneePolicy: TicketRule
  /** `null` = nobody (agents' unassigned tickets stay unassigned). */
  defaultAssignee: string | null
  /** False when the stored default is no longer a user here. */
  defaultAssigneeKnown: boolean
  people: TicketPerson[]
  canEdit: boolean
}

export interface TicketSettingsChange {
  briefPolicy?: TicketRule
  assigneePolicy?: TicketRule
  /** `''` clears it. */
  defaultAssignee?: string
}

const RULES: readonly TicketRule[] = ['off', 'warn', 'require']

function asRule(v: unknown): TicketRule {
  return RULES.includes(v as TicketRule) ? (v as TicketRule) : 'warn'
}

/** Shape a daemon body defensively (older servers have no route: the
 *  caller shows the error). */
export function parseTicketSettings(raw: unknown): TicketSettings {
  const o = (raw ?? {}) as Record<string, unknown>
  const people = Array.isArray(o.people)
    ? o.people
        .filter((p): p is Record<string, unknown> => !!p && typeof p === 'object')
        .map((p) => ({
          value: String(p.value ?? ''),
          label: String(p.label ?? p.value ?? ''),
          role: typeof p.role === 'string' ? p.role : null,
          disabled: p.disabled === true,
        }))
        .filter((p) => p.value !== '')
    : []
  return {
    briefPolicy: asRule(o.briefPolicy),
    assigneePolicy: asRule(o.assigneePolicy),
    defaultAssignee: typeof o.defaultAssignee === 'string' && o.defaultAssignee ? o.defaultAssignee : null,
    defaultAssigneeKnown: o.defaultAssigneeKnown !== false,
    people,
    canEdit: o.canEdit === true,
  }
}

export async function fetchTicketSettings(): Promise<TicketSettings> {
  return parseTicketSettings(await daemonCliGet<unknown>(primaryScope(), 'tickets/settings'))
}

export async function saveTicketSettings(change: TicketSettingsChange): Promise<TicketSettings> {
  return parseTicketSettings(await daemonCliPost<unknown>(primaryScope(), 'tickets/settings', change))
}

/** Picker rows for a rule (Off / Warn / Require). */
export const RULE_OPTIONS: { value: TicketRule; label: string }[] = [
  { value: 'off', label: 'Off' },
  { value: 'warn', label: 'Warn' },
  { value: 'require', label: 'Require' },
]
