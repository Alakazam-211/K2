// RS-A15 `guests.rooms` / `guests.setRooms`: the renderer half of the
// Garden transport (room-scope amendment 2026-10-18; one catalog row with
// the App route `/cli/skin/guests/rooms`).
//
// - Both verbs go to THIS computer's daemon as `POST /cli/zen/call`, like
//   the store verbs; the daemon checks the placement, the hash, the cap and
//   the maker's rights.
// - **Every write that changes something needs a person** (E4, like G8
//   `guests.invite`). The daemon answers the first call `confirm_required`
//   with the exact change and a `secretId`. This file asks K2 (never the
//   widget) to show that change on a sheet outside the frame. On Approve the
//   sheet takes the person's vault confirm (Touch ID, the login or vault
//   password) and hands back a token bound to that change; the call is sent
//   again with it. The widget never sees the change's token. Cancel answers
//   the widget `declined`.
// - One sheet at a time per frame: a second write while one is open is
//   `rate_limited` (a widget can't stack sheets on the person).

import type { CatalogErrorCode, CatalogWireError } from '../contract/catalog-types'
import { CATALOG_ERROR_CODES } from '../contract/catalog-types'

/** The verbs this file answers: RS-A15's two, plus the App's ceiling and
 *  guest list (prd-app-caps-0464-v1 AC3, catalog v22; reads, no sheet). */
export const ZEN_GUEST_VERBS: ReadonlySet<string> = new Set(['guests.rooms', 'guests.setRooms', 'app.rooms', 'guests.list'])

/** The vault confirm action the daemon binds a Garden guest-rooms write to. */
export const ZEN_GUESTS_CONFIRM_ACTION = 'garden-guests' as const

/** The change the daemon names in `confirm_required` (shown on the sheet). */
export interface ZenGuestRoomsChange {
  app: string
  appName: string
  appHandle: string
  username: string
  role: string | null
  before: string[]
  beforeHandles: string[]
  after: string[]
  afterHandles: string[]
  widget: string
  madeBy: string
}

/** What the sheet is asked to confirm. */
export interface ZenGuestRoomsAsk {
  change: ZenGuestRoomsChange
  secretId: string
}

export interface ZenCustomGuestsDeps {
  /** `POST /cli/zen/call` for this placement, with the person's token when K2 has one. */
  call(verb: string, args: unknown[], confirmToken?: string): Promise<unknown>
  /** Show K2's sheet outside the frame; resolves with the vault token, or null (Cancel). */
  confirm(ask: ZenGuestRoomsAsk): Promise<string | null>
}

export interface ZenCustomGuests {
  call(verb: string, args: unknown[]): Promise<unknown>
}

const KNOWN: ReadonlySet<string> = new Set(CATALOG_ERROR_CODES)

/** A refusal in the shared error list (the bridge passes it to the frame). */
export class ZenGuestsError extends Error {
  readonly wire: CatalogWireError
  readonly daemonCode: string
  /** `confirm_required`'s payload (never passed to the frame). */
  readonly ask: ZenGuestRoomsAsk | null
  constructor(daemonCode: string, message: string, ask: ZenGuestRoomsAsk | null = null) {
    super(message)
    this.name = 'ZenGuestsError'
    this.daemonCode = daemonCode
    const code = (KNOWN.has(daemonCode) ? daemonCode : 'failed') as CatalogErrorCode
    this.wire = { code, message }
    this.ask = ask
  }
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function strings(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []
}

function changeOf(v: unknown): ZenGuestRoomsChange | null {
  if (!isObj(v) || typeof v.app !== 'string' || typeof v.username !== 'string') return null
  return {
    app: v.app,
    appName: typeof v.appName === 'string' ? v.appName : v.app,
    appHandle: typeof v.appHandle === 'string' ? v.appHandle : v.app,
    username: v.username,
    role: typeof v.role === 'string' ? v.role : null,
    before: strings(v.before),
    beforeHandles: strings(v.beforeHandles),
    after: strings(v.after),
    afterHandles: strings(v.afterHandles),
    widget: typeof v.widget === 'string' ? v.widget : '',
    madeBy: typeof v.madeBy === 'string' ? v.madeBy : '',
  }
}

/** The daemon's `{"error": {...}}` envelope (the daemon client throws it as text). */
export function zenGuestsErrorFrom(err: unknown): ZenGuestsError {
  if (err instanceof ZenGuestsError) return err
  const text = err instanceof Error ? err.message : String(err)
  let env: unknown = null
  try {
    env = JSON.parse(text)
  } catch {
    env = null
  }
  const e = isObj(env) ? env.error : undefined
  if (isObj(e)) {
    const code = typeof e.code === 'string' ? e.code : 'failed'
    const message = typeof e.hint === 'string' && e.hint ? e.hint : code
    const change = code === 'confirm_required' ? changeOf(e.change) : null
    const ask = change && typeof e.secretId === 'string' ? { change, secretId: e.secretId } : null
    return new ZenGuestsError(code, message, ask)
  }
  return new ZenGuestsError('failed', text || 'guest rooms call failed')
}

export function createZenCustomGuests(deps: ZenCustomGuestsDeps): ZenCustomGuests {
  let sheetOpen = false
  return {
    async call(verb, args) {
      if (!ZEN_GUEST_VERBS.has(verb)) throw new ZenGuestsError('unknown_verb', `${verb} isn’t a guests verb`)
      try {
        return await deps.call(verb, args)
      } catch (raw) {
        const err = zenGuestsErrorFrom(raw)
        // Only a write waits for the person; the widget never sees the ask.
        if (verb !== 'guests.setRooms' || err.daemonCode !== 'confirm_required' || !err.ask) throw err
        if (sheetOpen) throw new ZenGuestsError('rate_limited', 'K2 is already asking the person about another change')
        sheetOpen = true
        let token: string | null
        try {
          token = await deps.confirm(err.ask)
        } finally {
          sheetOpen = false
        }
        if (!token) throw new ZenGuestsError('declined', 'The person didn’t approve this change.')
        try {
          return await deps.call(verb, args, token)
        } catch (again) {
          throw zenGuestsErrorFrom(again)
        }
      }
    },
  }
}
