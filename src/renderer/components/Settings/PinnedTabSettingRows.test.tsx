// @vitest-environment jsdom
// Workspace settings → Tabs (Rosson 2026-10-29), fail loud: "Show the Mobile
// tab" shows what the room does now (auto = on where the server can run a
// phone), a flip stores 0 or 1 through /cli/workspace/set and mirrors it into
// the window's projects list, and a server that can't run a phone shows it
// off and greyed. "Show the Design tab" sits beside it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliPost: vi.fn(async () => ({ ok: true })) }))
vi.mock('@/stores/projects', async () => {
  const { create } = await import('zustand')
  return {
    useProjectsStore: create<{ projects: { id: string; path: string; mobileTabEnabled?: number | null }[] }>(() => ({
      projects: [{ id: 'p1', path: '/w/app', mobileTabEnabled: null }],
    })),
  }
})

import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { PinnedTabSettingRow, WorkspaceTabsSettings } from './PinnedTabSettingRows'

function scope(canRunPhone: boolean): ServerScope {
  return {
    id: 'primary',
    isPrimary: true,
    isRemote: false,
    serverSupports: (f: string) => (f === 'mobile-sim-v1' ? canRunPhone : true),
  } as unknown as ServerScope
}

function mobileSwitch(): HTMLElement {
  return screen.getByRole('switch', { name: 'Show the Mobile tab' })
}

beforeEach(() => {
  vi.mocked(daemonCliPost).mockClear()
  useProjectsStore.setState({ projects: [{ id: 'p1', path: '/w/app', mobileTabEnabled: null }] as never })
})

afterEach(() => cleanup())

describe('Show the Mobile tab', () => {
  it('auto on a server that can run a phone reads on; flipping stores 0', async () => {
    render(<PinnedTabSettingRow kind="mobile" scope={scope(true)} project={{ id: 'p1', path: '/w/app', mobileTabEnabled: null }} />)
    expect(mobileSwitch().getAttribute('aria-checked')).toBe('true')
    await act(async () => {
      fireEvent.click(mobileSwitch())
    })
    expect(vi.mocked(daemonCliPost)).toHaveBeenCalledWith(expect.objectContaining({ isPrimary: true }), 'workspace/set', {
      project: '/w/app',
      fields: { mobile_tab_enabled: '0' },
    })
    expect(useProjectsStore.getState().projects[0].mobileTabEnabled).toBe(0)
  })

  it('off reads off; flipping stores 1 (never auto)', async () => {
    render(<PinnedTabSettingRow kind="mobile" scope={scope(true)} project={{ id: 'p1', path: '/w/app', mobileTabEnabled: 0 }} />)
    expect(mobileSwitch().getAttribute('aria-checked')).toBe('false')
    await act(async () => {
      fireEvent.click(mobileSwitch())
    })
    expect(vi.mocked(daemonCliPost)).toHaveBeenCalledWith(expect.anything(), 'workspace/set', {
      project: '/w/app',
      fields: { mobile_tab_enabled: '1' },
    })
  })

  it("a server that can't run a phone: off, greyed, says why, writes nothing", async () => {
    render(<PinnedTabSettingRow kind="mobile" scope={scope(false)} project={{ id: 'p1', path: '/w/app', mobileTabEnabled: 1 }} />)
    const sw = mobileSwitch()
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect((sw as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText("This server can't run a phone, so there is no Mobile tab here.")).toBeTruthy()
    await act(async () => {
      fireEvent.click(sw)
    })
    expect(vi.mocked(daemonCliPost)).not.toHaveBeenCalled()
  })

  it('a failed write shows the error and changes nothing', async () => {
    vi.mocked(daemonCliPost).mockImplementationOnce(async () => {
      throw new Error('forbidden')
    })
    render(<PinnedTabSettingRow kind="mobile" scope={scope(true)} project={{ id: 'p1', path: '/w/app', mobileTabEnabled: null }} />)
    await act(async () => {
      fireEvent.click(mobileSwitch())
    })
    expect(screen.getByRole('alert').textContent).toBe('forbidden')
    expect(useProjectsStore.getState().projects[0].mobileTabEnabled).toBeNull()
  })
})

describe('the Tabs group', () => {
  it('holds both switches under the projects.tabs anchor the toast opens', () => {
    const { container } = render(
      <WorkspaceTabsSettings scope={scope(true)} project={{ id: 'p1', path: '/w/app', designModeEnabled: 0, mobileTabEnabled: null }} />,
    )
    const group = container.querySelector('[data-settings-id="projects.tabs"]')
    if (!group) throw new Error('no Tabs anchor')
    expect(group.textContent).toContain('Tabs')
    expect(screen.getByRole('switch', { name: 'Show the Design tab' }).getAttribute('aria-checked')).toBe('false')
    expect(mobileSwitch().getAttribute('aria-checked')).toBe('true')
  })
})
