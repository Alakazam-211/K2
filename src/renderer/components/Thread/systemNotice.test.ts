// K2's own Thread notices are read by one function, `parseSystemNotice`:
// only a post stamped `author: {kind: "system"}` that carries a
// `systemNotice` is one (`notice` is the mail card's field, not this); its
// words lose the leading "K2: " or "[K2] "; its kind is info, delivery or
// action; its buttons come from `systemNotice.actions`: Send again (a
// daemon route to post to, done once `doneAt` is set) and open_terminal
// (a step the client takes, for the agent `workspace` names).
import { describe, expect, it } from 'vitest'
import { isSystemNoticeItem, parseSystemNotice, systemNoticeRoute, stripSystemPrefix } from './systemNotice'
import {
  ACTION_NOTICE,
  DELIVERY_NOTICES,
  INFO_NOTICES,
  LOOKALIKE_AGENT_POST,
  MAIL_NOTICE_CARD,
  OLD_DAEMON_NOTICE,
  RESEND_ACTION,
  resendAction,
} from './systemNotice.fixtures'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'

function withDoc(base: OverlayThreadItem, doc: Record<string, unknown>): OverlayThreadItem {
  return { ...base, doc: { ...base.doc, ...doc } as OverlayThreadItem['doc'] }
}

/** A delivery notice whose own actions are `actions`. */
function withActions(actions: unknown[]): OverlayThreadItem {
  return withDoc(DELIVERY_NOTICES.neverReadIn, { systemNotice: { kind: 'delivery', code: 'delivery.not_taken.never_read_in', actions } })
}

describe('parseSystemNotice', () => {
  it('reads the daemon\'s delivery notice: kind, code, words without "K2: ", Send again and Open its terminal', () => {
    const n = parseSystemNotice(DELIVERY_NOTICES.neverReadIn)
    expect(n).not.toBeNull()
    expect(n!.kind).toBe('delivery')
    expect(n!.code).toBe('delivery.not_taken.never_read_in')
    expect(n!.text.startsWith('k2-design didn\'t take the message "Got tokens back."')).toBe(true)
    expect(n!.actions).toEqual([
      { id: 'resend', label: 'Send again', call: { kind: 'post', route: 'thread/resend', body: { thread: 'k2-design', doc: 'n-never' } }, done: null },
      { id: 'open_terminal', label: 'Open its terminal', call: { kind: 'open_terminal', workspace: 'k2-design' }, done: null },
    ])
  })

  it('reads every known notice', () => {
    for (const it of Object.values(DELIVERY_NOTICES)) expect(parseSystemNotice(it)?.kind).toBe('delivery')
    for (const it of Object.values(INFO_NOTICES)) expect(parseSystemNotice(it)?.kind).toBe('info')
    expect(parseSystemNotice(ACTION_NOTICE)?.kind).toBe('action')
    expect(parseSystemNotice(INFO_NOTICES.jobRemoved)?.text).toBe('job **nightly-backup** removed by ada.')
    // "K2: K2 itself restarted…" keeps the second K2: only the lead goes.
    expect(parseSystemNotice(DELIVERY_NOTICES.restartMidDelivery)?.text.startsWith('K2 itself restarted')).toBe(true)
  })

  it('hides either text lead, "K2: " or "[K2] "', () => {
    expect(parseSystemNotice(INFO_NOTICES.jobRemoved)?.text).toBe('job **nightly-backup** removed by ada.')
    expect(parseSystemNotice(INFO_NOTICES.landed)?.text).toBe('Landed 2 commits from design/notices on main (a1b2c3d..d4e5f6a) for grader-a11y.')
  })

  it('is never a notice without the system stamp, whatever the words or fields say', () => {
    expect(parseSystemNotice(LOOKALIKE_AGENT_POST)).toBeNull()
    expect(parseSystemNotice(OLD_DAEMON_NOTICE)).toBeNull()
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { author: { kind: 'agent', login: 'k2-design' } }))).toBeNull()
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { author: { kind: 'System', login: 'k2' } }))).toBeNull()
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { author: 'system' }))).toBeNull()
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { author: null }))).toBeNull()
  })

  it('needs the systemNotice field: a stamped post without it, or with only `notice`, is not one', () => {
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { systemNotice: undefined }))).toBeNull()
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { systemNotice: [1, 2] }))).toBeNull()
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { systemNotice: 'delivery' }))).toBeNull()
    // The guessed field from before the daemon's: ignored.
    const guessed = withDoc(DELIVERY_NOTICES.neverReadIn, {
      systemNotice: undefined,
      notice: { kind: 'delivery', code: 'not_taken.never_read_in', actions: [RESEND_ACTION] },
    })
    expect(parseSystemNotice(guessed)).toBeNull()
    expect(isSystemNoticeItem(guessed)).toBe(false)
  })

  it("leaves K2's cards as cards: the mail notice and the changed-files card", () => {
    expect(parseSystemNotice(MAIL_NOTICE_CARD)).toBeNull()
    expect(isSystemNoticeItem(MAIL_NOTICE_CARD)).toBe(false)
    const changes = withDoc(INFO_NOTICES.jobAdded, { kind: 'changes', systemNotice: { kind: 'info', code: 'turn.changes' } })
    expect(parseSystemNotice(changes)).toBeNull()
  })

  it('reads an unknown kind as info, and a missing code as none', () => {
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { systemNotice: { kind: 'shout' } }))).toMatchObject({ kind: 'info', code: '', actions: [] })
  })

  it('keeps only buttons it knows and can carry out', () => {
    const n = parseSystemNotice(
      withActions([
        RESEND_ACTION,
        { id: 'mystery', label: 'Do a thing' }, // no route and not a client step
        { id: 'later', label: 'Later', route: '/cli/thread/later' }, // an id this client doesn't know
        { id: 'resend', label: '', route: '/cli/thread/resend' }, // no label
        { label: 'No id', route: '/cli/thread/resend' },
        { id: 'resend', label: 'Away', route: 'https://evil.test/cli/x' },
        { id: 'resend', label: 'Up', route: '/cli/../boot-status' },
        { id: 'resend', label: 'Query', route: '/cli/thread/resend?token=x' },
        { id: 'resend', label: 'No route' },
        'resend',
        null,
      ]),
    )
    expect(n?.actions.map((a) => a.label)).toEqual(['Send again'])
  })

  it('takes a body only when it is an object', () => {
    const n = parseSystemNotice(withActions([{ ...RESEND_ACTION, body: 'x' }]))
    expect(n?.actions[0].call).toEqual({ kind: 'post', route: 'thread/resend', body: {} })
  })

  it('open_terminal names its workspace and never posts, even with a route', () => {
    const n = parseSystemNotice(
      withActions([
        { id: 'open_terminal', label: 'Open its terminal', workspace: ' ledger ' },
        { id: 'open_terminal', label: 'Open it', route: '/cli/thread/resend' },
      ]),
    )
    expect(n?.actions.map((a) => a.call)).toEqual([
      { kind: 'open_terminal', workspace: 'ledger' },
      { kind: 'open_terminal', workspace: null },
    ])
  })

  it('reads a done action from doneAt and doneBy', () => {
    const n = parseSystemNotice(withActions([resendAction('n-never', { doneAt: 1_793_000_100, doneBy: { kind: 'user', login: 'ada' } })]))
    expect(n?.actions[0].done).toEqual({ at: 1_793_000_100, by: { kind: 'user', login: 'ada' } })
    // doneAt without a (readable) doneBy: done, by nobody named.
    expect(parseSystemNotice(withActions([resendAction('n-never', { doneAt: 1_793_000_100, doneBy: 'ada' })]))?.actions[0].done).toEqual({
      at: 1_793_000_100,
      by: null,
    })
    // No doneAt, or not a time: not done, whatever doneBy says.
    expect(parseSystemNotice(withActions([{ ...RESEND_ACTION, doneBy: { kind: 'user', login: 'ada' } }]))?.actions[0].done).toBeNull()
    expect(parseSystemNotice(withActions([{ ...RESEND_ACTION, doneAt: '1793000100' }]))?.actions[0].done).toBeNull()
  })

  it('draws a deleted notice as the usual deleted line, not a notice', () => {
    expect(parseSystemNotice(withDoc(INFO_NOTICES.jobAdded, { deleted: true, body: null }))).toBeNull()
  })
})

describe('systemNoticeRoute', () => {
  it('takes /cli/thread/resend or a bare route, and no host, parent step or query', () => {
    expect(systemNoticeRoute('/cli/thread/resend')).toBe('thread/resend')
    expect(systemNoticeRoute('thread/resend')).toBe('thread/resend')
    expect(systemNoticeRoute('/v1/hire')).toBeNull()
    expect(systemNoticeRoute('//evil.test/cli/x')).toBeNull()
    expect(systemNoticeRoute('https://evil.test/cli/thread/resend')).toBeNull()
    expect(systemNoticeRoute('/cli/thread/../x')).toBeNull()
    expect(systemNoticeRoute('/cli/thread/resend?doc=x')).toBeNull()
    expect(systemNoticeRoute('')).toBeNull()
    expect(systemNoticeRoute(7)).toBeNull()
  })
})

describe('stripSystemPrefix', () => {
  it('drops one leading "K2: " or "[K2] " lead', () => {
    expect(stripSystemPrefix('K2: job removed.')).toBe('job removed.')
    expect(stripSystemPrefix('K2:  two spaces')).toBe('two spaces')
    expect(stripSystemPrefix('K2: K2 restarted')).toBe('K2 restarted')
    expect(stripSystemPrefix('[K2] Retired grader-a11y.')).toBe('Retired grader-a11y.')
    expect(stripSystemPrefix('[K2] K2: both')).toBe('K2: both')
    expect(stripSystemPrefix('no label')).toBe('no label')
    expect(stripSystemPrefix('K2 restarted')).toBe('K2 restarted')
    expect(stripSystemPrefix('[K2]Landed')).toBe('Landed')
  })
})
