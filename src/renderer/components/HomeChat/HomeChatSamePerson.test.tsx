// @vitest-environment jsdom
// "Same person as …?" (prd-home-people-channels-v1 §6.3, K2 Design pass): a
// real two-answer dialog. Same person adds nothing (their row stays),
// Different person adds a second row, and Cancel NEVER adds a row.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    if (route === 'home-chat/status') return { ok: true, on: true, reason: null, flags: {} }
    if (route === 'home-chat/summary') return { ok: true, chat: 'live', convs: [] }
    if (route === 'home-chat/people')
      return { me: { ext: 'k2:owner', memberId: '1' }, people: [{ ext: 'k2:connect:a', username: 'alex', displayName: 'Alex Ng', ready: true, memberId: '100' }] }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async () => ({ ok: true })),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { AddPersonPanel, askSamePerson } from './HomeChatPickers'
import { resetHomeChatForTests, useHomeChatStore } from '@/stores/home-chat'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import type { Home, HomeRow } from '@/stores/homes'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

// Alex is already on this Home from another server (box-b).
const HOME: Home = { id: 'h1', name: 'Work', rows: [{ kind: 'person', address: 'k2:connect:b::box-b', workspaceId: null, label: 'Alex Ng' }] }

beforeEach(() => {
  resetHomeChatForTests()
  useHomeChatStore.getState().patchHost('box-b', {
    people: { me: { ext: 'k2:owner', memberId: '1' }, people: [{ ext: 'k2:connect:b', username: 'alex', displayName: 'Alex Ng', picture: null, role: null, ready: true, memberId: '9' }] },
  })
})
afterEach(() => {
  cleanup()
  useConfirmDialogStore.getState().close()
  resetHomeChatForTests()
})

async function pickAlex(): Promise<HomeRow[]> {
  const rows: HomeRow[] = []
  render(<AddPersonPanel home={HOME} onAddRow={(r) => (rows.push(r), true)} />)
  await act(async () => {
    fireEvent.click(await screen.findByText('Alex Ng'))
  })
  await waitFor(() => expect(useConfirmDialogStore.getState().isOpen).toBe(true))
  return rows
}

describe('same person as …?', () => {
  it('asks with two real answers, Same person first for Enter', async () => {
    await pickAlex()
    const st = useConfirmDialogStore.getState()
    expect(st.choices?.map((c) => c.label)).toEqual(['Different person', 'Same person'])
    expect(st.choices?.find((c) => c.primary)?.id).toBe('same')
    expect(st.title).toContain('Alex Ng')
  })

  it('Cancel adds nothing', async () => {
    const rows = await pickAlex()
    await act(async () => {
      useConfirmDialogStore.getState().close()
    })
    await new Promise((r) => setTimeout(r, 0))
    expect(rows).toEqual([])
  })

  it('Same person adds nothing (their row stays)', async () => {
    const rows = await pickAlex()
    await act(async () => {
      useConfirmDialogStore.getState().onChoose?.('same')
    })
    expect(rows).toEqual([])
  })

  it('Different person adds a second row', async () => {
    const rows = await pickAlex()
    await act(async () => {
      useConfirmDialogStore.getState().onChoose?.('different')
    })
    await waitFor(() => expect(rows).toEqual([{ kind: 'person', address: 'k2:connect:a::local', workspaceId: null, label: 'Alex Ng' }]))
  })

  it('askSamePerson maps any other answer to null', async () => {
    const p = askSamePerson('A', 'B', 'box')
    useConfirmDialogStore.getState().close()
    await expect(p).resolves.toBeNull()
  })
})
