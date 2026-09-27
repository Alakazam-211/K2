// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
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
    expect(icon.style.fontSize).toBe('14px')
    expect(icon.style.width).toBe('14px')
    expect(icon.style.height).toBe('14px')
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
    expect(icon.style.fontSize).toBe('14px')
    expect(icon.style.width).toBe('14px')
    expect(icon.style.height).toBe('14px')
    expect(icon.textContent).not.toContain('/work/notes.md')
    expect(agentIconsIn(fileRow)).toHaveLength(0)
    expect(globeIn(fileRow)).toBeNull()

    const termLabel = screen.getByText('claude')
    const termRow = termLabel.parentElement
    if (!termRow) throw new Error('terminal pane row did not render')
    expect(setiIn(termRow)).toBeNull()
    expect(agentIconsIn(termRow)).toHaveLength(0)
    expect(globeIn(termRow)).toBeNull()

    const webLabel = screen.getByText('example.com')
    const webRow = webLabel.parentElement
    if (!webRow) throw new Error('browser pane row did not render')
    if (!globeIn(webRow)) throw new Error('browser pane tab did not render a globe')
    expect(setiIn(webRow)).toBeNull()
    expect(agentIconsIn(webRow)).toHaveLength(0)
  })
})
