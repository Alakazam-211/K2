// @vitest-environment jsdom
// Back / forward live on the browser tab. Fail loud — no skip.
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ReactNode } from 'react'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { invoke } from '@tauri-apps/api/core'
import { BrowserPane } from './BrowserPane'
import { PaneGroupView } from '@/components/PaneLayout/PaneGroupView'
import { useTabsStore } from '@/stores/tabs'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))

vi.mock('@/kessel-term/TerminalPane', () => ({
  TerminalPane: () => <div data-testid="terminal-pane" />,
}))

vi.mock('@/components/Terminal/AlacrittyTerminalView', () => ({
  AlacrittyTerminalView: () => <div data-testid="alacritty-pane" />,
}))

vi.mock('@/components/FileViewerPane/FileViewerPane', () => ({
  FileViewerPane: () => <div data-testid="file-pane" />,
}))

vi.mock('@/components/AgentPane/AgentPane', () => ({
  AgentPane: () => <div data-testid="agent-pane" />,
}))

vi.mock('@/components/AgentCloseDialog/AgentCloseDialog', () => ({
  default: () => null,
}))

vi.mock('@/components/SessionView/AgentSessionChrome', () => ({
  AgentSessionChrome: ({ children }: { children?: ReactNode }) => children ?? null,
  useSidecarOverlayAddr: () => ({ title: '', addr: '' }),
}))

const here = dirname(fileURLToPath(import.meta.url))
const root = join(here, '../../../..')

class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

function resetTabs(): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
    navHistory: [],
    navIndex: -1,
  })
}

function renderActivePane(): void {
  const tab = useTabsStore.getState().tabs.at(-1)
  if (!tab) throw new Error('expected a tab to render')
  const paneGroupId = [...tab.paneGroups.keys()][0]
  if (!paneGroupId) throw new Error('expected a pane group to render')
  renderInPrimaryRoom(<PaneGroupView tabId={tab.id} paneGroupId={paneGroupId} />)
}

function expectNoBrowserArrows(): void {
  expect(screen.queryByRole('button', { name: 'Back' })).toBeNull()
  expect(screen.queryByRole('button', { name: 'Forward' })).toBeNull()
}

function asButton(el: HTMLElement): HTMLButtonElement {
  if (!(el instanceof HTMLButtonElement)) {
    throw new Error(`expected a button, got ${el.tagName}`)
  }
  return el
}

describe('browser tab back and forward', () => {
  beforeEach(() => {
    resetTabs()
    vi.mocked(invoke).mockReset()
    vi.mocked(invoke).mockImplementation(async () => null)
    globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
      x: 0,
      y: 32,
      width: 640,
      height: 480,
      top: 32,
      left: 0,
      right: 640,
      bottom: 512,
      toJSON() {
        return {}
      },
    } as DOMRect)
  })

  afterEach(() => {
    cleanup()
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('removes the top-bar arrows and keeps tab-strip history', () => {
    const topBar = readFileSync(join(root, 'src/renderer/components/TopBar/TopBar.tsx'), 'utf8')
    expect(topBar).not.toContain('NavButtons')
    expect(topBar).not.toContain('goBack')
    expect(topBar).not.toContain('goForward')
    expect(topBar).not.toContain('canGoBack')
    expect(topBar).not.toContain('canGoForward')

    const app = readFileSync(join(root, 'src/renderer/App.tsx'), 'utf8')
    expect(app).toContain('goBack()')
    expect(app).toContain('goForward()')

    const tabs = readFileSync(join(root, 'src/renderer/stores/tabs.ts'), 'utf8')
    expect(tabs).not.toContain('canGoBack')
    expect(tabs).not.toContain('canGoForward')
    expect(tabs).toContain('navHistory')
    expect(tabs).toContain('goBack:')
    expect(tabs).toContain('goForward:')

    const lib = readFileSync(join(root, 'src-tauri/src/lib.rs'), 'utf8')
    expect(lib).toContain('browser_back')
    expect(lib).toContain('browser_forward')
    expect(lib).toContain('browser_history_state')

    const rust = readFileSync(join(root, 'src-tauri/src/commands/browser_webviews.rs'), 'utf8')
    expect(rust.split('pub async fn browser_back').length - 1).toBe(2)
    expect(rust.split('pub async fn browser_forward').length - 1).toBe(2)
    expect(rust.split('pub async fn browser_history_state').length - 1).toBe(2)
    expect(rust).toContain('with_webview')
    expect(rust).not.toContain('with_back_forward_navigation_gestures')

    const caps = readFileSync(join(root, 'src-tauri/capabilities/default.json'), 'utf8')
    expect(caps).not.toContain('browser_back')
    expect(caps).not.toContain('browser_forward')
    expect(caps).not.toContain('browser_history_state')
  })

  it('starts disabled and does not call tab history', () => {
    const goBack = vi.fn()
    const goForward = vi.fn()
    useTabsStore.setState({ goBack, goForward })

    renderInPrimaryRoom(
      <BrowserPane itemId="fresh" tabId="tab-fresh" paneGroupId="pg-fresh" url="" />,
    )

    const back = screen.getByRole('button', { name: 'Back' })
    const forward = screen.getByRole('button', { name: 'Forward' })
    const address = screen.getByPlaceholderText('Enter URL…')
    expect(back.compareDocumentPosition(address) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(forward.compareDocumentPosition(address) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(asButton(back).disabled).toBe(true)
    expect(asButton(forward).disabled).toBe(true)
    fireEvent.click(back)
    fireEvent.click(forward)
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('browser_back', expect.anything())
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('browser_forward', expect.anything())
    expect(goBack).not.toHaveBeenCalled()
    expect(goForward).not.toHaveBeenCalled()
  })

  it('calls browser history commands, not tab history', async () => {
    vi.useFakeTimers()
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'browser_history_state') return { canBack: true, canForward: true }
      if (cmd === 'browser_current_url') return 'https://example.com/next'
      return null
    })
    const goBack = vi.fn()
    const goForward = vi.fn()
    useTabsStore.setState({ goBack, goForward })

    renderInPrimaryRoom(
      <BrowserPane
        itemId="b1"
        tabId="tab-1"
        paneGroupId="pg-1"
        url="https://example.com"
        standalone
      />,
    )

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      'browser_create',
      expect.objectContaining({ itemId: 'b1', parentWindow: 'main', url: 'https://example.com' }),
    )

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500)
    })

    const back = screen.getByRole('button', { name: 'Back' })
    const forward = screen.getByRole('button', { name: 'Forward' })
    expect(asButton(back).disabled).toBe(false)
    expect(asButton(forward).disabled).toBe(false)

    const beforeBack = vi.mocked(invoke).mock.calls.length
    fireEvent.click(back)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    const afterBack = vi.mocked(invoke).mock.calls.slice(beforeBack).map((call) => call[0])
    expect(afterBack[0]).toBe('browser_back')
    expect(afterBack).toContain('browser_current_url')
    expect(afterBack).toContain('browser_history_state')
    expect(afterBack).not.toContain('browser_navigate')
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('browser_back', {
      itemId: 'b1',
      parentWindow: 'main',
    })

    const beforeForward = vi.mocked(invoke).mock.calls.length
    fireEvent.click(forward)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    const afterForward = vi.mocked(invoke).mock.calls.slice(beforeForward).map((call) => call[0])
    expect(afterForward[0]).toBe('browser_forward')
    expect(afterForward).not.toContain('browser_navigate')
    expect(goBack).not.toHaveBeenCalled()
    expect(goForward).not.toHaveBeenCalled()
  })

  it('does not put browser arrows on a terminal tab', () => {
    useTabsStore.getState().addTab('/ws', { title: 'Shell' })
    renderActivePane()
    expect(screen.getByTestId('terminal-pane')).toBeTruthy()
    expectNoBrowserArrows()
  })

  it('does not put browser arrows on a file tab', () => {
    useTabsStore.getState().openFileInNewTab('/ws/notes.md')
    renderActivePane()
    expect(screen.getByTestId('file-pane')).toBeTruthy()
    expectNoBrowserArrows()
  })

  it('does not put browser arrows on an LLM tab', () => {
    useTabsStore.getState().openAgentPane('claude', '/ws', 'Claude')
    renderActivePane()
    expect(screen.getByTestId('agent-pane')).toBeTruthy()
    expectNoBrowserArrows()
    cleanup()
    resetTabs()
    useTabsStore.getState().addTab('/ws', { command: 'claude', title: 'Claude' })
    renderActivePane()
    expect(screen.getByTestId('terminal-pane')).toBeTruthy()
    expectNoBrowserArrows()
  })
})
