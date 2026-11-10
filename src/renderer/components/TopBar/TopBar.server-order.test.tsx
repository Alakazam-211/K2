// @vitest-environment jsdom
//
// Owner's request: top-left reads page selector, then server. Home hides the
// server (the centre title names a room's server there); every other page
// shows it.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: vi.fn(async () => ({ hasRunCommand: false })) }))
vi.mock('./ServerSwitcher', () => ({
  default: ({ triggerHidden }: { triggerHidden?: boolean }) => (
    <span data-stub="server" data-trigger-hidden={String(triggerHidden === true)} />
  ),
}))
vi.mock('./PageTabs', () => ({ default: () => <span data-stub="tabs" /> }))
vi.mock('./DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('./DesktopChromeRight', () => ({ default: () => null }))
vi.mock('./K2MarkButton', () => ({ default: () => <span data-stub="mark" /> }))
vi.mock('./TopBarUtilities', () => ({ default: () => null }))

import { act, cleanup, render } from '@testing-library/react'
import TopBar from './TopBar'
import { usePageViewStore, type AppPage } from '@/stores/page-view'

const stubs = (c: HTMLElement): (string | null)[] =>
  [...c.querySelectorAll('[data-stub]')].map((e) => e.getAttribute('data-stub'))
const hidden = (c: HTMLElement): string | null | undefined =>
  c.querySelector('[data-stub="server"]')?.getAttribute('data-trigger-hidden')

describe('top bar: selector, then server, hidden on Home', () => {
  afterEach(() => {
    cleanup()
    act(() => usePageViewStore.setState({ page: 'agents' }))
  })

  it('reads logo, page tabs, server', () => {
    const { container } = render(<TopBar />)
    expect(stubs(container)).toEqual(['mark', 'tabs', 'server'])
  })

  it('hides the trigger on Home only', () => {
    act(() => usePageViewStore.setState({ page: 'home' }))
    const { container } = render(<TopBar />)
    expect(hidden(container)).toBe('true')
    for (const page of ['agents', 'projects', 'feedback', 'wiki'] as AppPage[]) {
      act(() => usePageViewStore.setState({ page }))
      expect(hidden(container)).toBe('false')
    }
    act(() => usePageViewStore.setState({ page: 'home' }))
    expect(hidden(container)).toBe('true')
  })

  it.each([
    ['Settings', '../Settings/Settings.tsx'],
    ['Projects', '../Projects/ProjectsPage.tsx'],
    ['Wiki', '../Wiki/WikiPage.tsx'],
    ['Tickets', '../Feedback/FeedbackPage.tsx'],
  ])('%s bar: same order, server always shown', (_name, rel) => {
    const src = readFileSync(join(dirname(fileURLToPath(import.meta.url)), rel), 'utf8')
    const tabs = src.indexOf('<PageTabs />')
    expect(tabs).toBeGreaterThan(-1)
    expect(src.indexOf('<ServerSwitcher />')).toBeGreaterThan(tabs)
    expect(src).not.toContain('triggerHidden')
  })
})
