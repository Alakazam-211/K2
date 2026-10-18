// RS-A15: the Garden half of guests.rooms / guests.setRooms. A write the
// daemon says needs the person goes through K2's sheet (deps.confirm), never
// through the widget, and is sent again with the token the sheet got.

import { describe, expect, it, vi } from 'vitest'
import { createZenCustomGuests, ZenGuestsError, zenGuestsErrorFrom, ZEN_GUEST_VERBS, type ZenGuestRoomsAsk } from './zen-custom-guests'
import { ZEN_CUSTOM_VERBS } from './zen-verbs.generated'

const change = {
  app: 'svc-1',
  appName: 'jobs',
  appHandle: 'nsi/jobs',
  username: 'dana',
  role: null,
  before: [],
  beforeHandles: [],
  after: ['p-acme'],
  afterHandles: ['acme'],
  widget: 'guestdesk',
  madeBy: 'owner',
}

/** The daemon client throws the error envelope as the message text. */
function daemonError(code: string, extra: Record<string, unknown> = {}): Error {
  return new Error(JSON.stringify({ error: { code, hint: `${code} hint`, ...extra } }))
}

const confirmRequired = () => daemonError('confirm_required', { secretId: 'guests:abc', action: 'garden-guests', change })

describe('zen custom guests', () => {
  it('the catalog rows are the two verbs, under guests:rooms', () => {
    expect([...ZEN_GUEST_VERBS].sort()).toEqual(['guests.rooms', 'guests.setRooms'])
    const rows = ZEN_CUSTOM_VERBS as unknown as Record<string, { cap: string | null }>
    expect(rows['guests.rooms']?.cap).toBe('guests:rooms')
    expect(rows['guests.setRooms']?.cap).toBe('guests:rooms')
  })

  it('a read goes straight through, with no sheet', async () => {
    const call = vi.fn().mockResolvedValue({ ok: true, rooms: [] })
    const confirm = vi.fn()
    const g = createZenCustomGuests({ call, confirm })
    await expect(g.call('guests.rooms', [{ app: 'svc-1', username: 'dana' }])).resolves.toEqual({ ok: true, rooms: [] })
    expect(call).toHaveBeenCalledTimes(1)
    expect(call.mock.calls[0]).toEqual(['guests.rooms', [{ app: 'svc-1', username: 'dana' }]])
    expect(confirm).not.toHaveBeenCalled()
  })

  it('a write asks the person on K2’s sheet, then repeats the call with the token', async () => {
    const call = vi.fn().mockRejectedValueOnce(confirmRequired()).mockResolvedValueOnce({ ok: true, changed: true })
    let asked: ZenGuestRoomsAsk | null = null
    const confirm = vi.fn(async (ask: ZenGuestRoomsAsk) => {
      asked = ask
      return 'k2vc_token'
    })
    const g = createZenCustomGuests({ call, confirm })
    const args = [{ app: 'svc-1', username: 'dana', rooms: ['acme'] }]
    await expect(g.call('guests.setRooms', args)).resolves.toEqual({ ok: true, changed: true })
    expect(asked).toEqual({ secretId: 'guests:abc', change })
    expect(call.mock.calls[0]).toEqual(['guests.setRooms', args])
    expect(call.mock.calls[1]).toEqual(['guests.setRooms', args, 'k2vc_token'])
  })

  it('Cancel answers the widget declined, and nothing is sent again', async () => {
    const call = vi.fn().mockRejectedValue(confirmRequired())
    const g = createZenCustomGuests({ call, confirm: async () => null })
    const err = await g.call('guests.setRooms', [{ app: 'svc-1', username: 'dana', rooms: [] }]).catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ZenGuestsError)
    expect((err as ZenGuestsError).wire).toEqual({ code: 'declined', message: 'The person didn’t approve this change.' })
    expect(call).toHaveBeenCalledTimes(1)
  })

  it('one sheet at a time: a second write while one is open is rate_limited', async () => {
    let release: (t: string | null) => void = () => undefined
    const call = vi.fn().mockRejectedValue(confirmRequired())
    const g = createZenCustomGuests({ call, confirm: () => new Promise((r) => (release = r)) })
    const first = g.call('guests.setRooms', [{ app: 'svc-1', username: 'dana', rooms: [] }]).catch((e: unknown) => e)
    await Promise.resolve()
    await Promise.resolve()
    const second = await g.call('guests.setRooms', [{ app: 'svc-1', username: 'dana', rooms: [] }]).catch((e: unknown) => e)
    expect((second as ZenGuestsError).wire.code).toBe('rate_limited')
    release(null)
    expect(((await first) as ZenGuestsError).wire.code).toBe('declined')
  })

  it('a read never shows a sheet, and the ask never reaches the frame’s error', async () => {
    const g = createZenCustomGuests({ call: vi.fn().mockRejectedValue(confirmRequired()), confirm: vi.fn() })
    const err = (await g.call('guests.rooms', [{}]).catch((e: unknown) => e)) as ZenGuestsError
    expect(err.wire).toEqual({ code: 'confirm_required', message: 'confirm_required hint' })
    expect(JSON.stringify(err.wire)).not.toContain('guests:abc')
  })

  it('daemon refusals keep their shared code; unknown ones are failed', () => {
    expect(zenGuestsErrorFrom(daemonError('outside_app_rooms')).wire.code).toBe('outside_app_rooms')
    expect(zenGuestsErrorFrom(daemonError('grant_required')).wire.code).toBe('grant_required')
    const odd = zenGuestsErrorFrom(daemonError('widget_not_found'))
    expect(odd.wire.code).toBe('failed')
    expect(odd.daemonCode).toBe('widget_not_found')
    expect(zenGuestsErrorFrom(new Error('plain')).wire).toEqual({ code: 'failed', message: 'plain' })
  })
})
