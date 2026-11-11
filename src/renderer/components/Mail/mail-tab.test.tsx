// @vitest-environment jsdom
// OA5 + Rosson 2026-11-10: the Mail tab shows for Owner, Admin and Member
// logins (and the owner token, whose whoami says owner), and only when the
// server has mail (hosting on or a linked inbox, from
// GET /cli/mail/reader/presence). Never for a Voice & Chat only login or an
// unknown role. It follows the server live: `mail_changed` and window focus
// re-check it.
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'

interface Presence {
  hosting: boolean
  linkedInboxes: boolean
  visible: number
}

const h = vi.hoisted(() => ({
  role: null as string | null,
  presence: { hosting: false, linkedInboxes: false, visible: 0 } as Presence,
  mailHandlers: new Set<(reason: string) => void>(),
  presenceGets: 0,
}))

vi.mock('@/components/Presence/PresenceKickButton', () => ({
  fetchViewerRole: vi.fn(async () => h.role),
}))

vi.mock('@/kessel/server-scope', () => ({
  primaryScope: () => ({ id: 'primary' }),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route !== 'mail/reader/presence') throw new Error(`unexpected route ${route}`)
    h.presenceGets += 1
    return { ok: true, ...h.presence }
  }),
}))

vi.mock('@/stores/session-events', () => ({
  onMailChanged: (_scope: unknown, fn: (reason: string) => void) => {
    h.mailHandlers.add(fn)
    return () => h.mailHandlers.delete(fn)
  },
}))

import { __resetMailTabForTests, mailRoleAllows, mailTabShownFor, serverHasMail, useMailTabShown } from './mail-tab'

beforeEach(() => {
  __resetMailTabForTests()
  h.role = null
  h.presence = { hosting: false, linkedInboxes: false, visible: 0 }
  h.mailHandlers.clear()
  h.presenceGets = 0
})
afterEach(cleanup)

function Probe(): React.JSX.Element {
  return <span data-testid="shown">{String(useMailTabShown())}</span>
}

const shown = (): string | null => screen.getByTestId('shown').textContent

/** Render, wait for the first presence answer, and let it land. */
async function renderSettled(): Promise<void> {
  render(<Probe />)
  await waitFor(() => expect(h.presenceGets).toBeGreaterThan(0))
  await act(async () => {})
}

function mailChanged(reason: string): void {
  expect(h.mailHandlers.size).toBeGreaterThan(0)
  for (const fn of [...h.mailHandlers]) fn(reason)
}

describe('Mail tab gate', () => {
  it('roles: Owner, Admin and Member may; Voice & Chat only and no role may not', () => {
    expect(['owner', 'admin', 'member'].map(mailRoleAllows)).toEqual([true, true, true])
    expect(['voice_chat', 'viewer', ''].map(mailRoleAllows)).toEqual([false, false, false])
    expect(mailRoleAllows(null)).toBe(false)
  })

  it('the server rule: hosting or a linked inbox', () => {
    expect(serverHasMail(null)).toBe(false)
    expect(serverHasMail({ hosting: false, linkedInboxes: false, visible: 0 })).toBe(false)
    expect(serverHasMail({ hosting: true, linkedInboxes: false, visible: 0 })).toBe(true)
    expect(serverHasMail({ hosting: false, linkedInboxes: true, visible: 0 })).toBe(true)
    const mail = { hosting: true, linkedInboxes: true, visible: 4 }
    expect(mailTabShownFor('member', mail)).toBe(true)
    expect(mailTabShownFor(null, mail)).toBe(false)
    expect(mailTabShownFor('owner', { hosting: false, linkedInboxes: false, visible: 0 })).toBe(false)
  })

  it('is hidden on a server with no mail, even for the Owner', async () => {
    h.role = 'owner'
    await renderSettled()
    expect(shown()).toBe('false')
  })

  it('shows with hosted mail enabled', async () => {
    h.role = 'member'
    h.presence = { hosting: true, linkedInboxes: false, visible: 0 }
    await renderSettled()
    expect(shown()).toBe('true')
  })

  it('shows with a linked inbox and no hosting', async () => {
    h.role = 'admin'
    h.presence = { hosting: false, linkedInboxes: true, visible: 1 }
    await renderSettled()
    expect(shown()).toBe('true')
  })

  it('hides live when the last linked inbox is unlinked, and shows again when hosting turns on', async () => {
    h.role = 'owner'
    h.presence = { hosting: false, linkedInboxes: true, visible: 1 }
    await renderSettled()
    expect(shown()).toBe('true')

    h.presence = { hosting: false, linkedInboxes: false, visible: 0 }
    await act(async () => mailChanged('inboxes-changed'))
    await waitFor(() => expect(shown()).toBe('false'))

    h.presence = { hosting: true, linkedInboxes: false, visible: 0 }
    await act(async () => mailChanged('server-state-changed'))
    await waitFor(() => expect(shown()).toBe('true'))
  })

  it('re-checks on window focus', async () => {
    h.role = 'member'
    await renderSettled()
    expect(shown()).toBe('false')
    h.presence = { hosting: true, linkedInboxes: false, visible: 0 }
    await act(async () => {
      window.dispatchEvent(new Event('focus'))
    })
    await waitFor(() => expect(shown()).toBe('true'))
  })

  it('stays hidden for a Voice & Chat only login whatever mail the server has', async () => {
    h.role = null // fetchViewerRole reports voice_chat as no role
    h.presence = { hosting: true, linkedInboxes: true, visible: 3 }
    render(<Probe />)
    await act(async () => {})
    expect(shown()).toBe('false')
    await act(async () => mailChanged('inboxes-changed'))
    await act(async () => {
      window.dispatchEvent(new Event('focus'))
    })
    expect(shown()).toBe('false')
    expect(h.presenceGets).toBe(0)
  })

  it('an answer without the fields (an older daemon) keeps it hidden', async () => {
    h.role = 'owner'
    h.presence = {} as Presence
    await renderSettled()
    expect(shown()).toBe('false')
  })
})
