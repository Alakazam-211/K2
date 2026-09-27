// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { PaneTabBar } from '@/components/PaneLayout/PaneTabBar'
import { TabBar } from '@/components/TabBar/TabBar'
import { SetiFileIcon } from '@/lib/seti-file-icons'
import { resolveSetiIcon } from '@/lib/seti-file-icons/resolve'
import { useStyleStore } from '@/stores/style'
import { useTabsStore, type Item, type PaneGroup, type Tab } from '@/stores/tabs'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

function fileItem(filePath: string, pinned = false): Item {
  return {
    id: `file:${filePath}`,
    type: 'file-viewer',
    data: { filePath },
    pinned,
  }
}

function termItem(command?: string): Item {
  return {
    id: 'term-1',
    type: 'terminal',
    data: {
      terminalId: 'term-1',
      cwd: '/ws',
      ...(command ? { command } : {}),
    },
  }
}

function browserItem(url: string): Item {
  return {
    id: 'browser-1',
    type: 'browser',
    data: { url },
  }
}

function makeTab(id: string, title: string, items: Item[], extra?: Partial<Tab>): Tab {
  const pg: PaneGroup = { id: `${id}-pg`, items, activeItemIndex: 0 }
  return {
    id,
    title,
    mosaicTree: pg.id,
    paneGroups: new Map([[pg.id, pg]]),
    ...extra,
  }
}

function showTab(tab: Tab): HTMLElement {
  useTabsStore.setState({
    tabs: [tab],
    activeTabId: tab.id,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
  })
  render(<TabBar cwd="/ws" />)
  const el = document.querySelector(`[data-tab-id="${tab.id}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`tab ${tab.id} did not render`)
  return el
}

function setiIn(root: ParentNode): HTMLElement | null {
  return (
    Array.from(root.querySelectorAll('span')).find((el) =>
      el.style.fontFamily.includes('k2-seti'),
    ) ?? null
  )
}

function agentIconsIn(root: ParentNode): SVGElement[] {
  return Array.from(root.querySelectorAll('svg')).filter(
    (svg) => svg.getAttribute('viewBox') === '0 0 24 24',
  )
}

function globeIn(root: ParentNode): SVGElement | null {
  return root.querySelector('circle[r="6.5"]')?.closest('svg') ?? null
}

function pinIn(root: ParentNode): SVGPathElement | null {
  return root.querySelector('svg path[d^="M9.828"]')
}

function glyphFor(name: string): string {
  const scheme = useStyleStore.getState().resolvedScheme === 'light' ? 'light' : 'dark'
  return String.fromCodePoint(resolveSetiIcon(name, scheme).code)
}

describe('tab file and browser icons', () => {
  beforeEach(() => {
    useTabsStore.setState({
      tabs: [],
      activeTabId: null,
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })
  })

  afterEach(() => {
    cleanup()
    useTabsStore.setState({
      tabs: [],
      activeTabId: null,
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })
  })

  it('renders Seti for a file-viewer tab and does not render AgentIcon', () => {
    // item.pinned is the recycle flag (openFileInNewTab). Not the pin glyph.
    const root = showTab(makeTab('file', 'Scratch', [fileItem('/work/notes.md', true)]))
    const icon = setiIn(root)
    if (!icon) throw new Error('file tab did not render a Seti icon')
    expect(icon.textContent).toBe(glyphFor('notes.md'))
    expect(icon.textContent).not.toBe(glyphFor('Scratch'))
    expect(icon.textContent).not.toContain('/work/notes.md')
    expect(icon.style.fontSize).toBe('16px')
    expect(icon.style.width).toBe('16px')
    expect(icon.style.height).toBe('16px')
    expect(icon.getAttribute('title')).toBeNull()
    expect(root.querySelector('span.truncate')?.textContent).toBe('Scratch')
    expect(agentIconsIn(root)).toHaveLength(0)
    expect(globeIn(root)).toBeNull()
    expect(pinIn(root)).toBeNull()

    cleanup()
    render(<SetiFileIcon name="notes.md" />)
    const drawer = setiIn(document.body)
    if (!drawer) throw new Error('default SetiFileIcon did not render')
    expect(drawer.style.fontSize).toBe('16px')
    expect(drawer.style.width).toBe('16px')
    expect(drawer.style.height).toBe('16px')
  })

  it('keeps AgentIcon when a command is set, even with a file-viewer in the tab', () => {
    const root = showTab(
      makeTab('llm', 'Claude', [fileItem('/work/notes.md', true), termItem('claude')]),
    )
    const agents = agentIconsIn(root)
    if (agents.length !== 1) throw new Error('claude tab did not render AgentIcon')
    expect(agents[0].querySelector('title')?.textContent).toBe('Claude')
    expect(setiIn(root)).toBeNull()
    expect(globeIn(root)).toBeNull()
    expect(pinIn(root)).toBeNull()
    expect(root.querySelector('span.truncate')?.textContent).toBe('Claude')
  })

  it('renders the globe for a browser-only tab and not Seti', () => {
    const root = showTab(makeTab('web', 'example.com', [browserItem('https://example.com')]))
    const globe = globeIn(root)
    if (!globe) throw new Error('browser tab did not render a globe')
    const cls = globe.getAttribute('class') ?? ''
    expect(cls).toContain('w-3')
    expect(cls).toContain('h-3')
    expect(cls).toContain('text-[var(--color-text-muted)]')
    expect(cls).toContain('opacity-70')
    expect(cls).not.toContain('text-[var(--color-accent)]')
    expect(setiIn(root)).toBeNull()
    expect(agentIconsIn(root)).toHaveLength(0)
    expect(pinIn(root)).toBeNull()
    expect(root.querySelector('span.truncate')?.textContent).toBe('example.com')
  })

  it('uses a window icon for a plain shell and keeps the harness mark for Claude', () => {
    const shell = showTab(makeTab('sh', 'zsh', [termItem('zsh')]))
    if (!shell.querySelector('[data-shell-tab-icon]')) throw new Error('plain shell tab has no window icon')
    expect(agentIconsIn(shell)).toHaveLength(0)
    cleanup()

    const empty = showTab(makeTab('empty', 'Terminal', [termItem()]))
    if (!empty.querySelector('[data-shell-tab-icon]')) throw new Error('empty shell tab has no window icon')
    cleanup()

    const claude = showTab(makeTab('c', 'Claude', [termItem('claude')]))
    expect(claude.querySelector('[data-shell-tab-icon]')).toBeNull()
    expect(agentIconsIn(claude).length).toBeGreaterThan(0)
    cleanup()
  })

  it('renders the pin for a pinned HTML tab', () => {
    const root = showTab(
      makeTab('pin', 'page.html', [fileItem('/work/page.html', true)], { isPinnedFile: true }),
    )
    if (!pinIn(root)) throw new Error('pinned HTML tab did not render the pin')
    expect(setiIn(root)).toBeNull()
    expect(globeIn(root)).toBeNull()
    expect(agentIconsIn(root)).toHaveLength(0)
    expect(root.querySelector('span.truncate')?.textContent).toBe('page.html')
  })

  it('renders Seti on a file pane tab and not on a terminal pane tab', () => {
    render(
      <PaneTabBar
        items={[
          { id: 'f', type: 'file-viewer', data: { filePath: '/work/notes.md' } },
          { id: 't', type: 'terminal', data: { terminalId: 't1', cwd: '/ws', command: 'claude' } },
          { id: 'b', type: 'browser', data: { url: 'https://example.com' } },
        ]}
        activeItemIndex={0}
        onActivate={() => {}}
        onClose={() => {}}
      />,
    )

    const fileLabel = screen.getByText('notes.md')
    const fileRow = fileLabel.parentElement
    if (!fileRow) throw new Error('file pane row did not render')
    const icon = setiIn(fileRow)
    if (!icon) throw new Error('file pane tab did not render Seti')
    expect(icon.textContent).toBe(glyphFor('notes.md'))
    expect(icon.style.fontSize).toBe('16px')
    expect(icon.style.width).toBe('16px')
    expect(icon.style.height).toBe('16px')
    expect(icon.textContent).not.toContain('/work/notes.md')
    expect(agentIconsIn(fileRow)).toHaveLength(0)
    expect(globeIn(fileRow)).toBeNull()

    const termLabel = screen.getByText('claude')
    const termRow = termLabel.parentElement
    if (!termRow) throw new Error('terminal pane row did not render')
    expect(setiIn(termRow)).toBeNull()
    expect(agentIconsIn(termRow)).toHaveLength(0)
    expect(termRow.querySelector('[data-shell-tab-icon]')).toBeNull()
    expect(globeIn(termRow)).toBeNull()

    const webLabel = screen.getByText('example.com')
    const webRow = webLabel.parentElement
    if (!webRow) throw new Error('browser pane row did not render')
    if (!globeIn(webRow)) throw new Error('browser pane tab did not render a globe')
    expect(setiIn(webRow)).toBeNull()
    expect(agentIconsIn(webRow)).toHaveLength(0)

    cleanup()
    render(
      <PaneTabBar
        items={[{ id: 'z', type: 'terminal', data: { terminalId: 'z1', cwd: '/ws', command: 'zsh' } }]}
        activeItemIndex={0}
        onActivate={() => {}}
        onClose={() => {}}
      />,
    )
    const zsh = screen.getByText('zsh').parentElement
    if (!zsh?.querySelector('[data-shell-tab-icon]')) throw new Error('plain shell pane has no window icon')
  })

  it('paints a 16px data favicon on the strip and the pane row', () => {
    const icon = 'data:image/png;base64,AAAA'
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    const tab = useTabsStore.getState().tabs[0]
    const pg = Array.from(tab.paneGroups.values())[0]
    useTabsStore.getState().applyBrowserPageMeta(tab.id, pg.id, pg.items[0].id, {
      title: 'Example Domain',
      icon,
    })
    const root = document.querySelector(`[data-tab-id="${tab.id}"]`)
    render(<TabBar cwd="/ws" />)
    const strip = document.querySelector(`[data-tab-id="${tab.id}"]`)
    if (!(strip instanceof HTMLElement)) throw new Error('browser tab did not render')
    expect(root).toBeNull()
    const img = strip.querySelector('img')
    if (!(img instanceof HTMLImageElement)) throw new Error('browser tab did not render a favicon')
    expect(img.getAttribute('src')).toBe(icon)
    expect(img.style.width).toBe('16px')
    expect(img.style.height).toBe('16px')
    expect(img.style.borderRadius).toBe('0px')
    expect(img.className).not.toContain('rounded')
    expect(globeIn(strip)).toBeNull()
    expect(strip.querySelector('span.truncate')?.textContent).toBe('Example Domain')

    cleanup()
    const item = Array.from(useTabsStore.getState().tabs[0].paneGroups.values())[0].items[0]
    render(
      <PaneTabBar
        items={[item]}
        activeItemIndex={0}
        onActivate={() => {}}
        onClose={() => {}}
      />,
    )
    const row = screen.getByText('Example Domain').parentElement
    if (!row) throw new Error('pane row did not render the page title')
    const rowImg = row.querySelector('img')
    if (!(rowImg instanceof HTMLImageElement)) throw new Error('pane row did not render a favicon')
    expect(rowImg.getAttribute('src')).toBe(icon)
    expect(rowImg.style.width).toBe('16px')
    expect(rowImg.style.height).toBe('16px')
    expect(rowImg.style.borderRadius).toBe('0px')
    expect(globeIn(row)).toBeNull()
  })

  it('keeps the globe when the favicon is missing, https, or broken', () => {
    const missing = showTab(makeTab('web', 'example.com', [browserItem('https://example.com')]))
    const globe = globeIn(missing)
    if (!globe) throw new Error('missing icon did not stay the globe')
    const cls = globe.getAttribute('class') ?? ''
    expect(cls).toContain('w-3')
    expect(cls).toContain('h-3')
    expect(missing.querySelector('img')).toBeNull()

    cleanup()
    const https = showTab(makeTab('https', 'example.com', [{
      id: 'browser-https',
      type: 'browser',
      data: { url: 'https://example.com', icon: 'https://example.com/favicon.ico' },
    }]))
    if (!globeIn(https)) throw new Error('https favicon was not kept as the globe')
    expect(https.querySelector('img')).toBeNull()

    cleanup()
    const broken = showTab(makeTab('bad', 'example.com', [{
      id: 'browser-bad',
      type: 'browser',
      data: { url: 'https://example.com', title: 'Example', icon: 'data:image/png;base64,AAAA' },
    }]))
    const img = broken.querySelector('img')
    if (!(img instanceof HTMLImageElement)) throw new Error('data favicon did not render before error')
    fireEvent.error(img)
    if (!globeIn(broken)) throw new Error('broken favicon did not fall back to the globe')
    expect(broken.querySelector('img')).toBeNull()
  })

  it('does not put the favicon ahead of a file or terminal in a mixed tab', () => {
    const icon = 'data:image/png;base64,AAAA'
    const root = showTab(makeTab('mix', 'Notes', [
      browserItem('https://example.com'),
      fileItem('/work/notes.md'),
    ]))
    const live = useTabsStore.getState().tabs[0]
    const pg = Array.from(live.paneGroups.values())[0]
    const browser = pg.items.find((item) => item.type === 'browser')
    if (!browser) throw new Error('mixed tab lost the browser item')
    useTabsStore.getState().applyBrowserPageMeta(live.id, pg.id, browser.id, {
      title: 'Example Domain',
      icon,
    })
    cleanup()
    const again = document.querySelector(`[data-tab-id="${live.id}"]`)
    render(<TabBar cwd="/ws" />)
    const strip = document.querySelector(`[data-tab-id="${live.id}"]`)
    if (!(strip instanceof HTMLElement)) throw new Error('mixed tab did not render')
    expect(again).toBeNull()
    expect(strip.querySelector('img')).toBeNull()
    expect(globeIn(strip)).toBeNull()
    if (!setiIn(strip)) throw new Error('mixed tab did not keep the file glyph ahead of the favicon')
    expect(strip.querySelector('span.truncate')?.textContent).toBe('Notes')
    expect(root.querySelector('img')).toBeNull()
  })

  it('shows the page title on a locked tab pane row without changing the strip', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    const tab = useTabsStore.getState().tabs[0]
    const pg = Array.from(tab.paneGroups.values())[0]
    useTabsStore.getState().setTabTitle(tab.id, 'Kept', { locked: true })
    useTabsStore.getState().applyBrowserPageMeta(tab.id, pg.id, pg.items[0].id, {
      title: 'Example Domain',
    })
    render(<TabBar cwd="/ws" />)
    const strip = document.querySelector(`[data-tab-id="${tab.id}"]`)
    if (!(strip instanceof HTMLElement)) throw new Error('locked tab did not render')
    expect(strip.querySelector('span.truncate')?.textContent).toBe('Kept')
    expect(globeIn(strip)).not.toBeNull()

    cleanup()
    const item = Array.from(useTabsStore.getState().tabs[0].paneGroups.values())[0].items[0]
    render(
      <PaneTabBar
        items={[item]}
        activeItemIndex={0}
        onActivate={() => {}}
        onClose={() => {}}
      />,
    )
    if (!screen.getByText('Example Domain')) throw new Error('pane row did not show the page title')
    const row = screen.getByText('Example Domain').parentElement
    if (!row || !globeIn(row)) throw new Error('locked pane row did not keep the globe')
  })

  it('does not put the favicon ahead of a terminal', () => {
    const root = showTab(makeTab('mix-term', 'zsh', [
      {
        id: 'b',
        type: 'browser',
        data: { url: 'https://example.com', icon: 'data:image/png;base64,AAAA', title: 'Example Domain' },
      },
      termItem('zsh'),
    ]))
    expect(root.querySelector('img')).toBeNull()
    expect(globeIn(root)).toBeNull()
    if (!root.querySelector('[data-shell-tab-icon]')) throw new Error('terminal glyph lost to the favicon')
    expect(root.querySelector('span.truncate')?.textContent).toBe('zsh')
  })
})
