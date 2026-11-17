// K2's own Thread notices are read by one function, `parseSystemNotice`:
// only a post stamped `author: {kind: "system"}` is one; its words lose the
// leading "K2: "; its kind is info, delivery or action; its buttons come
// from `notice.actions`, each a daemon route to post to or a step the
// client takes itself (open_terminal).
import { describe, expect, it } from 'vitest'
import { parseSystemNotice, systemNoticeRoute, stripSystemPrefix } from './systemNotice'
import {
  ACTION_NOTICE,
  DELIVERY_NOTICES,
  INFO_NOTICES,
  LOOKALIKE_AGENT_POST,
  OLD_DAEMON_NOTICE,
  RESEND_ACTION,
} from './systemNotice.fixtures'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'

function withDoc(base: OverlayThreadItem, doc: Record<string, unknown>): OverlayThreadItem {
  return { ...base, doc: { ...base.doc, ...doc } as OverlayThreadItem['doc'] }
}

describe('parseSystemNotice', () => {
  it('reads a delivery notice: kind, code, words without "K2: ", and both buttons', () => {
    const n = parseSystemNotice(DELIVERY_NOTICES.neverReadIn)
    expect(n).not.toBeNull()
    expect(n!.kind).toBe('delivery')
    expect(n!.code).toBe('not_taken.never_read_in')
    expect(n!.text.startsWith('k2-design didn\'t take the message "Got tokens back."')).toBe(true)
    expect(n!.actions).toEqual([
      { id: 'resend', label: 'Send again', call: { kind: 'post', route: 'threads/resend', body: { thread: 'k2-design', doc: 'd-tokens' } } },
      { id: 'open_terminal', label: 'Open its terminal', call: { kind: 'open_terminal' } },
    ])
  })

  it('reads every known notice', () => {
    for (const it of Object.values(DELIVERY_NOTICES)) expect(parseSystemNotice(it)?.kind).toBe('delivery')
    for (const it of Object.values(INFO_NOTICES)) expect(parseSystemNotice(it)?.kind).toBe('info')
    expect(parseSystemNotice(ACTION_NOTICE)?.kind).toBe('action')
    expect(parseSystemNotice(INFO_NOTICES.jobRemoved)?.text).toBe('job **nightly-backup** removed by ada.')
    expect(parseSystemNotice(INFO_NOTICES.fileSaved)?.text).toBe('saved agent-orders/out/report.txt')
    // "K2: K2 itself restarted…" keeps the second K2: only the label goes.
    expect(parseSystemNotice(DELIVERY_NOTICES.restartMidDelivery)?.text.startsWith('K2 itself restarted')).toBe(true)
  })

  it('is never a notice without the system stamp, whatever the words or fields say', () => {
    expect(parseSystemNotice(LOOKALIKE_AGENT_POST)).toBeNull()
    expect(parseSystemNotice(OLD_DAEMON_NOTICE)).toBeNull()
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { author: { kind: 'agent', login: 'k2-design' } }))).toBeNull()
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { author: { kind: 'System' } }))).toBeNull()
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { author: 'system' }))).toBeNull()
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { author: null }))).toBeNull()
  })

  it('reads a stamped post with no notice, or an unknown kind, as info', () => {
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { notice: undefined }))).toMatchObject({ kind: 'info', code: '', actions: [] })
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { notice: { kind: 'shout' } }))?.kind).toBe('info')
    expect(parseSystemNotice(withDoc(DELIVERY_NOTICES.neverReadIn, { notice: [1, 2] }))?.kind).toBe('info')
  })

  it('keeps only buttons it can carry out', () => {
    const n = parseSystemNotice(
      withDoc(ACTION_NOTICE, {
        notice: {
          kind: 'action',
          actions: [
            RESEND_ACTION,
            { id: 'mystery', label: 'Do a thing' }, // no route and not a client step
            { id: 'x', label: '', route: '/cli/threads/resend' }, // no label
            { label: 'No id', route: '/cli/threads/resend' },
            { id: 'away', label: 'Away', route: 'https://evil.test/cli/x' },
            { id: 'up', label: 'Up', route: '/cli/../boot-status' },
            { id: 'q', label: 'Query', route: '/cli/threads/resend?token=x' },
            'resend',
            null,
          ],
        },
      }),
    )
    expect(n?.actions.map((a) => a.id)).toEqual(['resend'])
  })

  it('takes a body only when it is an object', () => {
    const n = parseSystemNotice(
      withDoc(ACTION_NOTICE, { notice: { kind: 'action', actions: [{ id: 'resend', label: 'Send again', route: 'threads/resend', body: 'x' }] } }),
    )
    expect(n?.actions[0].call).toEqual({ kind: 'post', route: 'threads/resend', body: {} })
  })

  it('draws a deleted notice as the usual deleted line, not a notice', () => {
    expect(parseSystemNotice(withDoc(INFO_NOTICES.jobAdded, { deleted: true, body: null }))).toBeNull()
  })
})

describe('systemNoticeRoute', () => {
  it('takes a /cli/ route or a bare one, and nothing else', () => {
    expect(systemNoticeRoute('/cli/threads/resend')).toBe('threads/resend')
    expect(systemNoticeRoute('threads/resend')).toBe('threads/resend')
    expect(systemNoticeRoute('/v1/hire')).toBeNull()
    expect(systemNoticeRoute('//evil.test/cli/x')).toBeNull()
    expect(systemNoticeRoute('/cli/threads/../x')).toBeNull()
    expect(systemNoticeRoute('')).toBeNull()
    expect(systemNoticeRoute(7)).toBeNull()
  })
})

describe('stripSystemPrefix', () => {
  it('drops one leading "K2: " label', () => {
    expect(stripSystemPrefix('K2: job removed.')).toBe('job removed.')
    expect(stripSystemPrefix('K2:  two spaces')).toBe('two spaces')
    expect(stripSystemPrefix('K2: K2 restarted')).toBe('K2 restarted')
    expect(stripSystemPrefix('no label')).toBe('no label')
    expect(stripSystemPrefix('K2 restarted')).toBe('K2 restarted')
  })
})
