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
  getFail: null as Error | null,
  hang: false,
  nightReads: 0,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    if (route === 'organization/night') {
      if (!cli.settings.orgChart) throw new Error('The Org Chart is off on this server')
      cli.nightReads++
      const n = (cli.settings.orgChartNight as { runAt?: string } | undefined)?.runAt ?? '01:00'
      return { runAt: n, spreadEndsAt: '04:00', spreadMinutes: 180, governor: { slots: 2 } }
    }
    if (route !== 'settings/get') throw new Error(`unexpected GET ${route}`)
    if (cli.getFail) throw cli.getFail
    return cli.settings
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string, body: unknown) => {
    cli.posts.push({ route, body })
    if (cli.hang) return new Promise(() => {})
    if (cli.fail) throw cli.fail
    cli.settings = { ...cli.settings, ...(body as Record<string, unknown>) }
    return {}
  }),
  isHostSwitchedError: () => false,
}))
const role = vi.hoisted(() => ({ current: 'owner' as string | null }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => role.current) }))

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { act } from 'react'
import { DEFAULT_NIGHT, nightProblem, OrgChartSection, spreadLine } from './OrgChartSection'
import { useSettingsStore } from '@/stores/settings'
import { DaemonHttpError } from '@/lib/daemon-http-error'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

afterEach(() => {
  cleanup()
  cli.posts = []
  cli.fail = null
  cli.getFail = null
  cli.hang = false
  cli.nightReads = 0
  cli.settings = { orgChart: false, ownerTimezone: 'America/Denver' }
  role.current = 'owner'
  act(() => useSettingsStore.setState({ orgChart: false }))
})

describe('Settings → Org Chart', () => {
  it('is off by default; turning it on writes orgChart and shows the tab at once', async () => {
    render(<OrgChartSection />)
    const sw = await screen.findByRole('switch', { name: 'Org Chart' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    expect(sw.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(sw)
    expect(useSettingsStore.getState().orgChart).toBe(true)
    await waitFor(() => expect(cli.posts).toEqual([{ route: 'settings/update', body: { orgChart: true } }]))
  })

  it('a refusal on permission puts the switch back and says who can change it, with no retry', async () => {
    cli.fail = new DaemonHttpError('Only an Owner or Admin can change that.', 403, 'role_required')
    render(<OrgChartSection />)
    const sw = await screen.findByRole('switch', { name: 'Org Chart' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    expect((await screen.findByRole('alert')).textContent).toBe('Couldn’t save. Only the server’s owner or an admin can change these.')
    expect(useSettingsStore.getState().orgChart).toBe(false)
  })

  it('says Saved beside the switch once the daemon takes it', async () => {
    render(<OrgChartSection />)
    const sw = await screen.findByRole('switch', { name: 'Org Chart' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    await waitFor(() => expect(screen.getByRole('status').textContent).toBe('Saved'))
  })

  it('says Saving… while the switch is on its way', async () => {
    cli.hang = true
    render(<OrgChartSection />)
    const sw = await screen.findByRole('switch', { name: 'Org Chart' })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    expect(screen.getByRole('status').textContent).toBe('Saving…')
    expect((sw as HTMLButtonElement).disabled).toBe(true)
  })

  it('shows a loading card, then a plain error with Try again when the settings fail to load', async () => {
    cli.getFail = new Error('settings.json is locked')
    render(<OrgChartSection />)
    const card = await screen.findByTestId('org-chart-load-error')
    expect(card.textContent).toContain('Couldn’t load the Org Chart settings')
    expect(card.textContent).toContain('The server said: settings.json is locked.')
    expect(screen.queryByRole('switch', { name: 'Org Chart' })).toBeNull()
    cli.getFail = null
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(await screen.findByRole('switch', { name: 'Org Chart' })).toBeTruthy()
  })

  it("names the owner's timezone, and falls back to the server's clock without one", async () => {
    render(<OrgChartSection />)
    await waitFor(() => expect(screen.getByTestId('org-chart-timezone').textContent).toContain('America/Denver'))
    cleanup()
    cli.settings = { orgChart: true }
    render(<OrgChartSection />)
    await waitFor(() => expect(screen.getByTestId('org-chart-timezone').textContent).toContain('this server’s own clock'))
    expect(useSettingsStore.getState().orgChart).toBe(true)
  })

  it('night times save in order within 22:00–07:00; out of order is refused before sending', async () => {
    render(<OrgChartSection />)
    const run = (await screen.findByLabelText('Night run')) as HTMLInputElement
    await waitFor(() => expect(run.disabled).toBe(false))
    expect(screen.queryByRole('button', { name: 'Save times' })).toBeNull()
    fireEvent.change(run, { target: { value: '05:00' } })
    expect(screen.getByText('The times go in order: night run, then last call, then the new day.')).toBeTruthy()
    expect((screen.getByRole('button', { name: 'Save times' }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(run, { target: { value: '23:30' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save times' }))
    await waitFor(() =>
      expect(cli.posts).toEqual([{ route: 'settings/update', body: { orgChartNight: { ...DEFAULT_NIGHT, runAt: '23:30' } } }]),
    )
    await waitFor(() => expect(screen.getByRole('status').textContent).toBe('Saved'))
    expect(screen.queryByRole('button', { name: 'Save times' })).toBeNull()
  })

  it('Save shows Saving… and waits while the times are on their way', async () => {
    cli.hang = true
    render(<OrgChartSection />)
    const run = (await screen.findByLabelText('Night run')) as HTMLInputElement
    await waitFor(() => expect(run.disabled).toBe(false))
    fireEvent.change(run, { target: { value: '23:30' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save times' }))
    const btn = (await screen.findByRole('button', { name: 'Saving…' })) as HTMLButtonElement
    expect(btn.disabled).toBe(true)
    expect(screen.getByText('Saving the new times…')).toBeTruthy()
  })

  it('after a save, the pacing line is read again and shows the new time', async () => {
    cli.settings = { orgChart: true, ownerTimezone: 'America/Denver' }
    render(<OrgChartSection />)
    expect((await screen.findByTestId('org-chart-spread')).textContent).toContain('(01:00–04:00)')
    const before = cli.nightReads
    fireEvent.change(screen.getByLabelText('Night run'), { target: { value: '00:30' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save times' }))
    await waitFor(() => expect(cli.nightReads).toBeGreaterThan(before))
    await waitFor(() => expect(screen.getByTestId('org-chart-spread').textContent).toContain('(00:30–04:00)'))
  })

  it('Undo changes puts the saved times back; a failed save says why and keeps the edit', async () => {
    render(<OrgChartSection />)
    const run = (await screen.findByLabelText('Night run')) as HTMLInputElement
    await waitFor(() => expect(run.disabled).toBe(false))
    fireEvent.change(run, { target: { value: '23:30' } })
    fireEvent.click(screen.getByRole('button', { name: 'Undo changes' }))
    expect(run.value).toBe('01:00')
    expect(screen.queryByRole('button', { name: 'Save times' })).toBeNull()
    cli.fail = new DaemonHttpError('settings.json is locked', 500, 'internal')
    fireEvent.change(run, { target: { value: '23:30' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save times' }))
    expect((await screen.findByRole('alert')).textContent).toBe(
      'Couldn’t save this change. Nothing changed; try again.The server said: settings.json is locked.',
    )
    expect(run.value).toBe('23:30')
    expect(screen.getByRole('button', { name: 'Save times' })).toBeTruthy()
  })

  it('a save refused on permission says who can change these and offers no Save to retry', async () => {
    cli.fail = new DaemonHttpError('only the owner or an admin may change server settings', 403, 'role_required')
    render(<OrgChartSection />)
    const run = (await screen.findByLabelText('Night run')) as HTMLInputElement
    await waitFor(() => expect(run.disabled).toBe(false))
    fireEvent.change(run, { target: { value: '23:30' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save times' }))
    expect((await screen.findByRole('alert')).textContent).toBe('Couldn’t save. Only the server’s owner or an admin can change these.')
    expect(screen.queryByRole('button', { name: 'Save times' })).toBeNull()
    expect(screen.getByRole('button', { name: 'Undo changes' })).toBeTruthy()
  })

  it("reads the daemon's dayStartsAt (and an older daemon's editionAt) and writes dayStartsAt", async () => {
    cli.settings = { orgChart: true, orgChartNight: { runAt: '00:30', lastCallAt: '03:00', dayStartsAt: '05:45' } }
    render(<OrgChartSection />)
    await waitFor(() => expect((screen.getByLabelText('New day') as HTMLInputElement).value).toBe('05:45'))
    await waitFor(() => expect((screen.getByLabelText('New day') as HTMLInputElement).disabled).toBe(false))
    fireEvent.change(screen.getByLabelText('New day'), { target: { value: '06:15' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save times' }))
    await waitFor(() =>
      expect(cli.posts).toEqual([
        { route: 'settings/update', body: { orgChartNight: { runAt: '00:30', lastCallAt: '03:00', dayStartsAt: '06:15' } } },
      ]),
    )
    cleanup()
    cli.settings = { orgChart: true, orgChartNight: { runAt: '01:00', lastCallAt: '04:30', editionAt: '06:30' } }
    render(<OrgChartSection />)
    await waitFor(() => expect((screen.getByLabelText('New day') as HTMLInputElement).value).toBe('06:30'))
  })

  it('the rule itself', () => {
    expect(nightProblem(DEFAULT_NIGHT)).toBeNull()
    expect(nightProblem({ runAt: '22:00', lastCallAt: '02:00', dayStartsAt: '07:00' })).toBeNull()
    expect(nightProblem({ ...DEFAULT_NIGHT, dayStartsAt: '08:00' })).toBe('New day must fall between 22:00 and 07:00.')
  })

  it("shows how the night run is paced, read only, from the daemon's numbers", async () => {
    cli.settings = { orgChart: true }
    render(<OrgChartSection />)
    expect((await screen.findByTestId('org-chart-spread')).textContent).toBe(
      'Night run spreads over 180 min (01:00–04:00), 2 at a time on this server.',
    )
    cleanup()
    cli.settings = { orgChart: false }
    render(<OrgChartSection />)
    await waitFor(() => expect((screen.getByRole('switch', { name: 'Org Chart' }) as HTMLButtonElement).disabled).toBe(false))
    expect(screen.queryByTestId('org-chart-spread')).toBeNull()
    expect(spreadLine({ runAt: '01:00', spreadEndsAt: '04:00', spreadMinutes: 180 })).toBe('Night run spreads over 180 min (01:00–04:00).')
    expect(spreadLine(null)).toBeNull()
  })

  it('a Member sees it read-only', async () => {
    role.current = 'member'
    render(<OrgChartSection />)
    expect((await screen.findByTestId('org-chart-member-line')).textContent).toBe(
      'Only the server’s owner or an admin can change these.',
    )
    expect(screen.getByText('View only')).toBeTruthy()
    expect((screen.getByRole('switch', { name: 'Org Chart' }) as HTMLButtonElement).disabled).toBe(true)
    // The times read as plain text, not as fields that don't answer.
    expect(screen.getByLabelText('Night run').tagName).toBe('SPAN')
    expect(screen.getByLabelText('Night run').textContent).toBe('01:00')
    expect(screen.queryByRole('button', { name: 'Save times' })).toBeNull()
  })
})
