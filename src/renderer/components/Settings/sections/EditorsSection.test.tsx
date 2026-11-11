// @vitest-environment jsdom
// Settings → Editors / Fonts → Defaults: the page copes with any reply from
// the editor scan (a null reply used to crash it) and says what Rescan did.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

const h = vi.hoisted(() => ({
  replies: {} as Record<string, unknown>,
  fail: {} as Record<string, string>,
}))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (h.fail[cmd]) throw new Error(h.fail[cmd])
    return h.replies[cmd]
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { EditorsSection, parseDetectedApps } from './EditorsSection'
import { useSettingsStore } from '@/stores/settings'

const app = (id: string, label: string, type: 'editor' | 'terminal', installed = true) => ({ id, label, macApp: `${label}.app`, cliCommand: id, installed, type })
const LIST = [app('cursor', 'Cursor', 'editor'), app('zed', 'Zed', 'editor'), app('sublime', 'Sublime Text', 'editor', false), app('iterm2', 'iTerm2', 'terminal')]

async function mount(): Promise<void> {
  await act(async () => {
    render(<EditorsSection />)
  })
}

beforeEach(() => {
  h.replies = { projects_get_all_editors: LIST, projects_refresh_editors: [...LIST, app('ghostty', 'Ghostty', 'terminal')] }
  h.fail = {}
  useSettingsStore.setState({ projectSettings: {} })
})
afterEach(() => cleanup())

describe('parseDetectedApps', () => {
  it('takes a list and refuses anything else', () => {
    expect(parseDetectedApps(LIST)).toHaveLength(4)
    expect(parseDetectedApps(null)).toBeNull()
    expect(parseDetectedApps({})).toBeNull()
    expect(parseDetectedApps([null, { id: 'x' }, LIST[0]])).toEqual([LIST[0]])
  })
})

describe('EditorsSection', () => {
  it('lists the apps, installed first, with what each is', async () => {
    await mount()
    expect(screen.getByTestId('editor-app-cursor').textContent).toContain('Installed')
    expect(screen.getByTestId('editor-app-sublime').textContent).toContain('Not installed')
    const ids = Array.from(document.querySelectorAll('[data-testid^="editor-app-"]')).map((e) => e.getAttribute('data-testid'))
    expect(ids).toEqual(['editor-app-cursor', 'editor-app-zed', 'editor-app-iterm2', 'editor-app-sublime'])
    expect(screen.getByRole('button', { name: 'Editor' }).textContent).toContain('Cursor')
    expect(screen.getByRole('button', { name: 'Terminal app' }).textContent).toContain('Terminal')
  })

  it('a null reply is an error with Try again, not a crash', async () => {
    h.replies.projects_get_all_editors = null
    await mount()
    expect(screen.getByTestId('editors-error').textContent).toContain('couldn’t check which apps are installed')
    h.replies.projects_get_all_editors = LIST
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    })
    expect(screen.queryByTestId('editors-error')).toBeNull()
    expect(screen.getByTestId('editor-app-cursor')).toBeTruthy()
  })

  it('a failed read shows the server’s reason', async () => {
    h.fail.projects_get_all_editors = 'daemon unreachable'
    await mount()
    expect(screen.getByTestId('editors-error').textContent).toContain('The server said: daemon unreachable.')
  })

  it('nothing installed is the empty state with Rescan', async () => {
    h.replies.projects_get_all_editors = [app('sublime', 'Sublime Text', 'editor', false)]
    await mount()
    expect(screen.getByTestId('editors-empty').textContent).toContain('No editors or terminal apps found')
    expect(screen.getByRole('button', { name: 'Rescan' })).toBeTruthy()
  })

  it('Rescan says what it found, or that it failed and the list is the last scan', async () => {
    await mount()
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Rescan' }))
    })
    expect(screen.getByTestId('editors-scan-note').textContent).toBe('Rescanned just now: 2 editors and 2 terminal apps installed.')
    expect(screen.getByTestId('editor-app-ghostty')).toBeTruthy()
    h.fail.projects_refresh_editors = 'scan timed out'
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Rescan' }))
    })
    expect(screen.getByTestId('editors-scan-note').textContent).toBe('Couldn’t rescan: scan timed out. The list shows the last scan; try again.')
    expect(screen.getByTestId('editor-app-ghostty')).toBeTruthy()
  })
})
