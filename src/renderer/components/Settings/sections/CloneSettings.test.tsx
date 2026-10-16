// @vitest-environment jsdom
// Clone permissions in Settings (prd-clone-carries-threads-and-vault-v1
// §4.2, D2): General's owner-only master switch (default ON) and Admin
// Access's per-person "Can clone" (Owner rows locked on; Owner-only).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const settingsGet = vi.hoisted(() => vi.fn())
const settingsUpdate = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet, daemonCliPost: vi.fn() }))
vi.mock('@/lib/daemon-settings', () => ({ settingsGet, settingsUpdate }))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => 'primary' }))

import {
  ClonePersonalComputersRow,
  canSetCloneSwitch,
  cloneSwitchValue,
  CLONE_SWITCH_LABEL,
  CLONE_SWITCH_OWNER_HINT,
} from './ClonePersonalComputersRow'
import { CanCloneToggle, canCloneState, setCanCloneOptimistic } from './CanCloneToggle'

beforeEach(() => {
  daemonCliGet.mockReset()
  settingsGet.mockReset()
  settingsUpdate.mockReset()
})
afterEach(() => cleanup())

describe('General → Allow cloning agents to people\'s personal computers', () => {
  it('defaults ON (absent key) and only the Owner may set it', () => {
    expect(cloneSwitchValue({})).toBe(true)
    expect(cloneSwitchValue({ allowClonePersonalComputers: false })).toBe(false)
    expect(canSetCloneSwitch({ owner: true })).toBe(true)
    expect(canSetCloneSwitch({ owner: false, role: 'owner' })).toBe(true)
    expect(canSetCloneSwitch({ owner: false, role: 'admin' })).toBe(false)
    expect(canSetCloneSwitch(null)).toBe(false)
  })

  it('the Owner flips it and the daemon answer is shown', async () => {
    daemonCliGet.mockResolvedValue({ owner: true, role: 'owner' })
    settingsGet.mockResolvedValue({})
    settingsUpdate.mockResolvedValue({ allowClonePersonalComputers: false })
    render(<ClonePersonalComputersRow scope={'primary' as never} />)
    const sw = screen.getByRole('switch', { name: CLONE_SWITCH_LABEL })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    expect(sw.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(sw)
    await waitFor(() => expect(settingsUpdate).toHaveBeenCalledWith({ allowClonePersonalComputers: false }))
    await waitFor(() => expect(sw.getAttribute('aria-checked')).toBe('false'))
  })

  it('an Admin sees it read-only with the owner hint', async () => {
    daemonCliGet.mockResolvedValue({ owner: false, role: 'admin' })
    settingsGet.mockResolvedValue({ allowClonePersonalComputers: false })
    render(<ClonePersonalComputersRow scope={'primary' as never} />)
    await waitFor(() => expect(screen.getByText(new RegExp(CLONE_SWITCH_OWNER_HINT))).toBeTruthy())
    const sw = screen.getByRole('switch', { name: CLONE_SWITCH_LABEL }) as HTMLButtonElement
    expect(sw.disabled).toBe(true)
    expect(sw.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(sw)
    expect(settingsUpdate).not.toHaveBeenCalled()
  })

  it('a refused write reverts the switch and shows why', async () => {
    daemonCliGet.mockResolvedValue({ owner: true })
    settingsGet.mockResolvedValue({ allowClonePersonalComputers: true })
    settingsUpdate.mockRejectedValue(new Error('settings_update 403: owner only'))
    render(<ClonePersonalComputersRow scope={'primary' as never} />)
    const sw = screen.getByRole('switch', { name: CLONE_SWITCH_LABEL })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    await waitFor(() => expect(screen.getByText(/owner only/)).toBeTruthy())
    expect(sw.getAttribute('aria-checked')).toBe('true')
  })
})

describe('Admin Access → Can clone', () => {
  it('Owner rows are on and locked; non-owner viewers cannot change any row', () => {
    expect(canCloneState('owner', false, 'owner')).toMatchObject({ checked: true, locked: true })
    expect(canCloneState('member', false, 'owner')).toMatchObject({ checked: false, locked: false })
    expect(canCloneState('member', undefined, 'owner')).toMatchObject({ checked: true, locked: false })
    expect(canCloneState('admin', true, 'admin')).toMatchObject({ checked: true, locked: true })
  })

  it('renders locked for an Owner row and calls onChange for a member row', () => {
    const onChange = vi.fn()
    render(
      <>
        <CanCloneToggle username="boss" rowRole="owner" canClone={true} myRole="owner" onChange={onChange} />
        <CanCloneToggle username="mo" rowRole="member" canClone={true} myRole="owner" onChange={onChange} />
      </>,
    )
    const boss = screen.getByTestId('can-clone-boss') as HTMLInputElement
    expect(boss.checked).toBe(true)
    expect(boss.disabled).toBe(true)
    fireEvent.click(screen.getByTestId('can-clone-mo'))
    expect(onChange).toHaveBeenCalledWith(false)
  })

  it('posts set-can-clone optimistically and reverts on a refusal', async () => {
    const applied: [string, boolean][] = []
    const onError = vi.fn()
    const refresh = vi.fn(async () => undefined)
    const post = vi.fn(async () => new Response(JSON.stringify({ error: 'nope' }), { status: 403 }))
    await setCanCloneOptimistic({
      username: 'mo',
      next: false,
      post,
      apply: (u, v) => applied.push([u, v]),
      errText: async (r) => ((await r.json()) as { error: string }).error,
      onError,
      refresh,
    })
    expect(post).toHaveBeenCalledWith('/set-can-clone', { username: 'mo', canClone: false })
    expect(applied).toEqual([
      ['mo', false],
      ['mo', true],
    ])
    expect(onError).toHaveBeenCalledWith('nope')
    expect(refresh).not.toHaveBeenCalled()

    const ok = vi.fn(async () => new Response('{"success":true}', { status: 200 }))
    applied.length = 0
    await setCanCloneOptimistic({ username: 'mo', next: true, post: ok, apply: (u, v) => applied.push([u, v]), errText: async () => '', onError, refresh })
    expect(applied).toEqual([['mo', true]])
    expect(refresh).toHaveBeenCalledTimes(1)
  })
})
