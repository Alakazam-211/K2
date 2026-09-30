// @vitest-environment jsdom
// Fail loud. Focus must not hide the native page. Cover and a hidden tab must.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import { invoke } from '@tauri-apps/api/core'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useWindowFocusStore } from '@/stores/window-focus'
import { BrowserPane, browserPaneVisible } from './BrowserPane'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))

class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

const NON_AGENTS: AppPage[] = ['projects', 'feedback', 'wiki']

function commandNames(): string[] {
  return vi.mocked(invoke).mock.calls.map((call) => String(call[0]))
}

function namesSince(start: number): string[] {
  return commandNames().slice(start)
}

async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}

function resetChrome(): void {
  useWindowFocusStore.setState({ isFocused: true })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.setState({ page: 'agents' })
}

describe('browserPaneVisible', () => {
  it('is visible when the tab is visible, the workspace is not covered, and the window is not focused', () => {
    expect(
      browserPaneVisible({
        standalone: false,
        tabVisible: true,
        settingsOpen: false,
        page: 'agents',
        windowFocused: false,
      }),
    ).toBe(true)
  })

  it('stays visible when that same pane is focused', () => {
    expect(
      browserPaneVisible({
        standalone: false,
        tabVisible: true,
        settingsOpen: false,
        page: 'agents',
        windowFocused: true,
      }),
    ).toBe(true)
  })

  it.each([false, true])(
    'is hidden when the tab is not visible (windowFocused=%s)',
    (windowFocused) => {
      expect(
        browserPaneVisible({
          standalone: false,
          tabVisible: false,
          settingsOpen: false,
          page: 'agents',
          windowFocused,
        }),
      ).toBe(false)
    },
  )

  it.each([false, true])(
    'is hidden when Settings covers the workspace (windowFocused=%s)',
    (windowFocused) => {
      expect(
        browserPaneVisible({
          standalone: false,
          tabVisible: true,
          settingsOpen: true,
          page: 'agents',
          windowFocused,
        }),
      ).toBe(false)
    },
  )

  it.each(
    NON_AGENTS.flatMap((page) =>
      [false, true].map((windowFocused) => ({ page, windowFocused })),
    ),
  )(
    'is hidden when $page covers the workspace (windowFocused=$windowFocused)',
    ({ page, windowFocused }) => {
      expect(
        browserPaneVisible({
          standalone: false,
          tabVisible: true,
          settingsOpen: false,
          page,
          windowFocused,
        }),
      ).toBe(false)
    },
  )

  it('leaves standalone embeds visible when covered and unfocused', () => {
    expect(
      browserPaneVisible({
        standalone: true,
        tabVisible: false,
        settingsOpen: true,
        page: 'projects',
        windowFocused: false,
      }),
    ).toBe(true)
  })
})

describe('browser show and hide commands', () => {
  beforeEach(() => {
    resetChrome()
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
    vi.restoreAllMocks()
    resetChrome()
  })

  it('shows without navigate while unfocused, and hides without close', async () => {
    useWindowFocusStore.setState({ isFocused: false })
    render(
      <BrowserPane itemId="b1" tabId="tab-1" paneGroupId="pg-1" url="https://example.com" />,
    )
    await flush()
    const created = commandNames()
    expect(created).toContain('browser_create')
    expect(created).not.toContain('browser_navigate')
    expect(created).not.toContain('browser_close')

    const beforeBlur = vi.mocked(invoke).mock.calls.length
    act(() => {
      useWindowFocusStore.setState({ isFocused: true })
    })
    act(() => {
      useWindowFocusStore.setState({ isFocused: false })
    })
    expect(namesSince(beforeBlur)).not.toContain('browser_set_visible')
    expect(namesSince(beforeBlur)).not.toContain('browser_navigate')
    expect(namesSince(beforeBlur)).not.toContain('browser_close')

    const beforeHide = vi.mocked(invoke).mock.calls.length
    act(() => {
      useSettingsStore.setState({ settingsOpen: true })
    })
    const hidden = namesSince(beforeHide)
    expect(hidden).toContain('browser_set_visible')
    expect(hidden).not.toContain('browser_navigate')
    expect(hidden).not.toContain('browser_close')
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      'browser_set_visible',
      { itemId: 'b1', visible: false, parentWindow: 'main' },
    )

    const beforeShow = vi.mocked(invoke).mock.calls.length
    act(() => {
      useSettingsStore.setState({ settingsOpen: false })
    })
    const shown = namesSince(beforeShow)
    expect(shown).toContain('browser_set_visible')
    expect(shown).not.toContain('browser_navigate')
    expect(shown).not.toContain('browser_close')
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      'browser_set_visible',
      { itemId: 'b1', visible: true, parentWindow: 'main' },
    )

    const beforePageHide = vi.mocked(invoke).mock.calls.length
    act(() => {
      usePageViewStore.setState({ page: 'projects' })
    })
    const pageHidden = namesSince(beforePageHide)
    expect(pageHidden).toContain('browser_set_visible')
    expect(pageHidden).not.toContain('browser_navigate')
    expect(pageHidden).not.toContain('browser_close')
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      'browser_set_visible',
      { itemId: 'b1', visible: false, parentWindow: 'main' },
    )
  })

  it.each([false, true])(
    'does not show a hidden tab or a covered workspace (windowFocused=%s)',
    async (focused) => {
      useWindowFocusStore.setState({ isFocused: focused })

      const hiddenTab = render(
        <TabVisibilityContext.Provider value={false}>
          <BrowserPane itemId="hidden" tabId="tab-h" paneGroupId="pg-h" url="https://example.com" />
        </TabVisibilityContext.Provider>,
      )
      await flush()
      expect(commandNames()).not.toContain('browser_create')
      expect(commandNames()).not.toContain('browser_navigate')
      hiddenTab.unmount()
      vi.mocked(invoke).mockClear()

      useSettingsStore.setState({ settingsOpen: true })
      const settings = render(
        <BrowserPane itemId="settings" tabId="tab-s" paneGroupId="pg-s" url="https://example.com" />,
      )
      await flush()
      expect(commandNames()).not.toContain('browser_create')
      expect(commandNames()).not.toContain('browser_set_visible')
      expect(commandNames()).not.toContain('browser_navigate')
      settings.unmount()
      vi.mocked(invoke).mockClear()
      useSettingsStore.setState({ settingsOpen: false })

      for (const page of NON_AGENTS) {
        usePageViewStore.setState({ page })
        const covered = render(
          <BrowserPane itemId={page} tabId={`tab-${page}`} paneGroupId={`pg-${page}`} url="https://example.com" />,
        )
        await flush()
        expect(commandNames()).not.toContain('browser_create')
        expect(commandNames()).not.toContain('browser_set_visible')
        expect(commandNames()).not.toContain('browser_navigate')
        covered.unmount()
        vi.mocked(invoke).mockClear()
      }
    },
  )
})

// V21 / D3 — with the same Browser tab open in two windows, one window's
// navigation reaches the other through the shared layout (`url` prop). The
// other window's live page must stay where it is; only this window's own
// request (a `navSeq` bump from openUrlInPane) moves it.
describe("another window's navigation does not move this page", () => {
  beforeEach(() => {
    resetChrome()
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
    vi.restoreAllMocks()
    resetChrome()
  })

  it('a remote url change does not call browser_navigate; a navSeq bump does', async () => {
    const view = render(
      <BrowserPane itemId="b2" tabId="tab-2" paneGroupId="pg-2" url="https://example.com/a" />,
    )
    await flush()
    expect(commandNames()).toContain('browser_create')

    // The other window navigated; its save changed the stored url only.
    const beforeRemote = vi.mocked(invoke).mock.calls.length
    view.rerender(
      <BrowserPane itemId="b2" tabId="tab-2" paneGroupId="pg-2" url="https://example.com/b" />,
    )
    await flush()
    const remote = namesSince(beforeRemote)
    expect(remote).not.toContain('browser_navigate')
    expect(remote).not.toContain('browser_close')
    expect(remote).not.toContain('browser_create')

    // This window asked (openUrlInPane bumps navSeq): navigate.
    const beforeLocal = vi.mocked(invoke).mock.calls.length
    view.rerender(
      <BrowserPane itemId="b2" tabId="tab-2" paneGroupId="pg-2" url="https://example.com/c" navSeq={1} />,
    )
    await flush()
    expect(namesSince(beforeLocal)).toContain('browser_navigate')
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('browser_navigate', {
      itemId: 'b2',
      url: 'https://example.com/c',
      parentWindow: 'main',
    })
  })

  it('a standalone embed (Settings OAuth) still follows its url prop', async () => {
    const view = render(
      <BrowserPane itemId="oauth" tabId="settings-oauth" paneGroupId="settings-oauth" url="https://example.com/start" standalone />,
    )
    await flush()
    expect(commandNames()).toContain('browser_create')
    const before = vi.mocked(invoke).mock.calls.length
    view.rerender(
      <BrowserPane itemId="oauth" tabId="settings-oauth" paneGroupId="settings-oauth" url="https://example.com/next" standalone />,
    )
    await flush()
    expect(namesSince(before)).toContain('browser_navigate')
  })
})
