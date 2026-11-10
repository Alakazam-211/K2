// @vitest-environment jsdom
// Settings → Org Chart: the switch (off by default) writes `orgChart` to the
// daemon and shows the tab at once; a refusal puts it back; the night times
// are in the owner's timezone and keep their order; a Member reads only.
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
const cli = vi.hoisted(() => ({
  settings: { orgChart: false, ownerTimezone: 'America/Denver' } as Record<string, unknown>,
  posts: [] as { route: string; body: unknown }[],
  fail: null as Error | null,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    if (route !== 'settings/get') throw new Error(`unexpected GET ${route}`)
    return cli.settings
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string, body: unknown) => {
    cli.posts.push({ route, body })
    if (cli.fail) throw cli.fail
    return {}
  }),
  isHostSwitchedError: () => false,
}))
const role = vi.hoisted(() => ({ current: 'owner' as string | null }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => role.current) }))

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { act } from 'react'
import { DEFAULT_NIGHT, nightProblem, OrgChartSection } from './OrgChartSection'
import { useSettingsStore } from '@/stores/settings'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

afterEach(() => {
  cleanup()
  cli.posts = []
  cli.fail = null
  cli.settings = { orgChart: false, ownerTimezone: 'America/Denver' }
  role.current = 'owner'
  act(() => useSettingsStore.setState({ orgChart: false }))
})

describe('Settings → Org Chart', () => {
  it('is off by default; turning it on writes orgChart and shows the tab at once', async () => {
    render(<OrgChartSection />)
    const sw = screen.getByRole('switch', { name: 'Org Chart' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    expect(sw.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(sw)
    expect(useSettingsStore.getState().orgChart).toBe(true)
    await waitFor(() => expect(cli.posts).toEqual([{ route: 'settings/update', body: { orgChart: true } }]))
  })

  it("a refusal puts the switch back and says the daemon's words", async () => {
    cli.fail = new Error('Only an Owner or Admin can change that.')
    render(<OrgChartSection />)
    const sw = screen.getByRole('switch', { name: 'Org Chart' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    expect(await screen.findByRole('alert')).toHaveProperty('textContent', 'Only an Owner or Admin can change that.')
    expect(useSettingsStore.getState().orgChart).toBe(false)
  })

  it("names the owner's timezone, and falls back to the server's clock without one", async () => {
    render(<OrgChartSection />)
    await waitFor(() => expect(screen.getByTestId('org-chart-timezone').textContent).toContain('America/Denver'))
    cleanup()
    cli.settings = { orgChart: true }
    render(<OrgChartSection />)
    await waitFor(() => expect(screen.getByTestId('org-chart-timezone').textContent).toContain("this server's own clock"))
    expect(useSettingsStore.getState().orgChart).toBe(true)
  })

  it('night times save in order within 22:00–07:00; out of order is refused before sending', async () => {
    render(<OrgChartSection />)
    const run = screen.getByLabelText('Night run') as HTMLInputElement
    await waitFor(() => expect(run.disabled).toBe(false))
    fireEvent.change(run, { target: { value: '05:00' } })
    expect(screen.getByText('The times go in order: night run, then last call, then the new day.')).toBeTruthy()
    expect((screen.getByRole('button', { name: 'Save times' }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(run, { target: { value: '23:30' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save times' }))
    await waitFor(() =>
      expect(cli.posts).toEqual([{ route: 'settings/update', body: { orgChartNight: { ...DEFAULT_NIGHT, runAt: '23:30' } } }]),
    )
  })

  it('the rule itself', () => {
    expect(nightProblem(DEFAULT_NIGHT)).toBeNull()
    expect(nightProblem({ runAt: '22:00', lastCallAt: '02:00', editionAt: '07:00' })).toBeNull()
    expect(nightProblem({ ...DEFAULT_NIGHT, editionAt: '08:00' })).toBe('New day must fall between 22:00 and 07:00.')
  })

  it('a Member sees it read-only', async () => {
    role.current = 'member'
    render(<OrgChartSection />)
    expect(await screen.findByTestId('org-chart-member-line')).toBeTruthy()
    expect((screen.getByRole('switch', { name: 'Org Chart' }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByLabelText('Night run') as HTMLInputElement).disabled).toBe(true)
  })
})
