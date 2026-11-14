// @vitest-environment jsdom
// The members sheet (K2 Design pass): K2's dialog shape, avatar rows with
// roles, and Invite (the daemon's channels/add) for someone in the channel.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
const h = vi.hoisted(() => ({
  post: vi.fn(async (_s: unknown, _r: string, _b?: unknown): Promise<unknown> => ({ ok: true, results: [] })),
  open: false,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    if (route === 'home-chat/people')
      return {
        me: { ext: 'k2:owner', memberId: '1' },
        people: [
          { ext: 'k2:connect:ana', username: 'ana', displayName: 'Ana Ruiz', role: 'admin', ready: true, memberId: '2' },
          { ext: 'k2:connect:ben', username: 'ben', displayName: 'Ben Okafor', role: 'member', ready: true, memberId: '3' },
          { ext: 'k2:connect:cy', username: 'cy', displayName: 'Cy Lee', role: 'member', ready: true, memberId: '4' },
        ],
      }
    if (route === 'home-chat/channels/members')
      return {
        conv: '700', slug: 'crit', state: 'active', name: 'design-crit', open: h.open, creator: 'k2:connect:ana', allowAgents: false,
        people: [{ ext: 'k2:owner', state: 'member' }, { ext: 'k2:connect:ana', state: 'member' }, { ext: 'k2:connect:ben', state: 'pending_add' }],
        agents: [{ name: 'Quill' }],
      }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: (s: unknown, r: string, b?: unknown) => h.post(s, r, b),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { HomeChatMembersSheet, memberRoleLabel } from './HomeChatMembersSheet'
import { resetHomeChatForTests } from '@/stores/home-chat'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const TARGET = { kind: 'channel' as const, hostKey: 'local', conv: '700', address: '700::local' }

beforeEach(() => {
  resetHomeChatForTests()
  h.post.mockClear()
  h.open = false
})
afterEach(() => {
  cleanup()
  resetHomeChatForTests()
})

async function open(onClose = vi.fn()) {
  render(<HomeChatMembersSheet target={TARGET} onClose={onClose} onChanged={vi.fn()} />)
  await screen.findByText('design-crit')
  await waitFor(() => expect(screen.getByText('Ana Ruiz')).toBeTruthy())
  return { onClose }
}

describe('members sheet', () => {
  it('roles read Owner and Admin; a member says nothing', () => {
    expect(memberRoleLabel('owner')).toBe('Owner')
    expect(memberRoleLabel('admin')).toBe('Admin')
    expect(memberRoleLabel('member')).toBeNull()
    expect(memberRoleLabel(null)).toBeNull()
  })

  it("is K2's dialog: the channel's lock glyph and name, invite only and the count", async () => {
    await open()
    const sheet = screen.getByTestId('home-chat-members')
    expect(sheet.getAttribute('role')).toBe('dialog')
    expect(sheet.closest('[data-k2-chamfer-float]')).not.toBeNull()
    expect(sheet.querySelector('[data-channel-glyph="lock"]')).not.toBeNull()
    expect(sheet.querySelector('[data-home-chat-members-sub]')?.textContent).toBe('Invite only · 3 people')
  })

  it('each person is an avatar row with their role, the maker and who is still being added', async () => {
    await open()
    const rows = [...screen.getByTestId('home-chat-member-list').querySelectorAll('li')]
    expect(rows.map((r) => r.querySelector('[data-home-chat-avatar]')?.textContent)).toEqual(['O', 'AR', 'BO'])
    expect(rows.map((r) => r.querySelector('[data-member-role]')?.textContent ?? '')).toEqual(['Owner', 'Admin · Created it', 'Adding…'])
    // The avatar takes the agent avatar's shape (round in Glass, cut in Terminal).
    expect(rows[1].querySelector('[data-home-chat-avatar]')?.hasAttribute('data-k2-avatar')).toBe(true)
  })

  it('Invite people lists who can be added, and Invite adds them through the daemon', async () => {
    await open()
    await act(async () => {
      fireEvent.click(screen.getByTestId('home-chat-add-people'))
    })
    const list = screen.getByTestId('home-chat-add-people-list')
    expect(list.textContent).toContain('Cy Lee')
    expect(list.textContent).not.toContain('Ana Ruiz')
    await act(async () => {
      fireEvent.click(list.querySelector('input[type="checkbox"]')!)
    })
    await act(async () => {
      fireEvent.click(screen.getByTestId('home-chat-invite-confirm'))
    })
    expect(h.post).toHaveBeenCalledWith(expect.anything(), 'home-chat/channels/add', { conv: '700', ext: ['k2:connect:cy'] })
  })

  it('an open channel shows the # glyph; Esc closes the sheet', async () => {
    h.open = true
    const { onClose } = await open()
    expect(screen.getByTestId('home-chat-members').querySelector('[data-channel-glyph="hash"]')).not.toBeNull()
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(onClose).toHaveBeenCalled()
  })
})
