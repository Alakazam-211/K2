// @vitest-environment jsdom
//
// T2.1 (prd-home-picker-and-remote-avatars-v1 S2, vs-live P25/P35): the
// Tickets / Wiki "All workspaces" dropdown, driven through the DOM. Landed
// on the pre-extraction code so moving its body onto the shared
// `SearchableAgentList` is proven not to change how Tickets and Wiki look
// or act: search by name and path, ↓↓ Enter, Esc that never reaches the
// page, the All / Unlinked / Projects rows, and the class lists of one row
// of each kind (inline snapshots — a changed class fails).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
const cli = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async () => ({ found: false, dataUrl: null })),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: cli.daemonCliGet,
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_scope: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import {
  UNLINKED_FILTER_VALUE,
  WorkspaceFilterDropdown,
  type FilterableWorkspace,
} from './WorkspaceFilterDropdown'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useProjectGroupsStore } from '@/stores/project-groups'
import type { ProjectGroup } from '@/components/Projects/projects-api'

const PNG = 'data:image/png;base64,iVBORw0KGgo='

// jsdom has neither CSS.escape nor scrollIntoView; the dropdown uses both
// for its keyboard row. Minimal stand-ins (the webview has the real ones).
if (typeof globalThis.CSS === 'undefined') {
  ;(globalThis as { CSS?: unknown }).CSS = { escape: (v: string) => v.replace(/["\\]/g, '\\$&') }
}
const scrolled = vi.fn()
Element.prototype.scrollIntoView = scrolled

const projects: FilterableWorkspace[] = [
  { id: 'w-alpha', name: 'Alpha', path: '/w/alpha', color: '#e06c75', iconUrl: PNG, focusGroupId: 'g1' },
  { id: 'w-beta', name: 'Beta', path: '/srv/deep/beta', color: '#61afef', iconUrl: null, focusGroupId: null },
  { id: 'w-gamma', name: 'Gamma', path: '/w/gamma', color: '#98c379', iconUrl: null, focusGroupId: 'g1' },
]

function projectGroup(id: string, name: string, memberCount: number): ProjectGroup {
  return {
    id,
    name,
    pocWorkspaceId: null,
    pinned: false,
    color: null,
    sortOrder: 0,
    createdAt: 0,
    updatedAt: 0,
    memberCount,
  }
}

beforeEach(() => {
  useFocusGroupsStore.setState({ focusGroups: [], focusGroupsEnabled: false })
  useProjectGroupsStore.setState({ groups: [projectGroup('pg1', 'Launch', 2)] })
  cli.daemonCliGet.mockClear()
})

afterEach(() => {
  cleanup()
})

/** Render the dropdown, open it, and let the avatar lookups settle. */
async function openDropdown(props: {
  value?: string
  showUnlinked?: boolean
  onChange?: (v: string) => void
}): Promise<{ onChange: ReturnType<typeof vi.fn>; input: HTMLInputElement; popover: HTMLElement }> {
  const onChange = vi.fn(props.onChange)
  render(
    <WorkspaceFilterDropdown
      projects={projects}
      value={props.value ?? 'all'}
      onChange={onChange}
      showUnlinked={props.showUnlinked ?? true}
    />,
  )
  fireEvent.click(screen.getByTitle('Filter by workspace or project'))
  const input = screen.getByPlaceholderText('Search workspaces...')
  if (!(input instanceof HTMLInputElement)) throw new Error('search is not an input')
  const popover = input.closest('.absolute')
  if (!(popover instanceof HTMLElement)) throw new Error('no popover around the search')
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0))
  })
  return { onChange, input, popover }
}

function rowFor(value: string): HTMLElement {
  const el = document.querySelector(`[data-ws-filter-value=${JSON.stringify(value)}]`)
  if (!(el instanceof HTMLElement)) throw new Error(`no row for ${value}`)
  return el
}

function rowValues(): string[] {
  return Array.from(document.querySelectorAll('[data-ws-filter-value]')).map(
    (el) => el.getAttribute('data-ws-filter-value') ?? '',
  )
}

/** Tag + class + inline style of an element and every descendant, one
 *  line each, indented — the look of one row, without its attributes. */
function classTree(el: Element, depth = 0): string {
  const style = el.getAttribute('style')
  const line = `${'  '.repeat(depth)}${el.tagName.toLowerCase()} .${el.getAttribute('class') ?? ''}${style ? ` {${style}}` : ''}`
  return [line, ...Array.from(el.children).map((c) => classTree(c, depth + 1))].join('\n')
}

describe('Tickets "All workspaces" dropdown — DOM (T2.1)', () => {
  it('typing filters rows by name and by path; All shows only with an empty query', async () => {
    const { input } = await openDropdown({})
    expect(rowValues()).toEqual(['all', UNLINKED_FILTER_VALUE, 'project:pg1', 'w-alpha', 'w-beta', 'w-gamma'])

    fireEvent.change(input, { target: { value: 'gam' } })
    expect(rowValues()).toEqual(['w-gamma'])

    fireEvent.change(input, { target: { value: '/srv/deep' } })
    expect(rowValues()).toEqual(['w-beta'])

    fireEvent.change(input, { target: { value: 'launch' } })
    expect(rowValues()).toEqual(['project:pg1'])

    fireEvent.change(input, { target: { value: 'unlinked' } })
    expect(rowValues()).toEqual([UNLINKED_FILTER_VALUE])

    fireEvent.change(input, { target: { value: 'zzz' } })
    expect(rowValues()).toEqual([])
    expect(screen.getByText('No workspaces match')).toBeTruthy()

    fireEvent.change(input, { target: { value: '' } })
    expect(rowValues()[0]).toBe('all')
  })

  it('↓↓ Enter picks the second value and closes', async () => {
    const { input, onChange } = await openDropdown({})
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(onChange).toHaveBeenCalledTimes(1)
    expect(onChange).toHaveBeenCalledWith(UNLINKED_FILTER_VALUE)
    expect(screen.queryByPlaceholderText('Search workspaces...')).toBeNull()
  })

  it('arrows clamp at the ends and walk into a filtered list', async () => {
    const { input, onChange } = await openDropdown({})
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(onChange).toHaveBeenCalledWith('all')
    cleanup()

    const second = await openDropdown({})
    fireEvent.change(second.input, { target: { value: 'a' } })
    // 'a' matches Alpha, Beta, Gamma, Launch, Unlinked (no All row).
    for (let i = 0; i < 20; i++) fireEvent.keyDown(second.input, { key: 'ArrowDown' })
    fireEvent.keyDown(second.input, { key: 'Enter' })
    expect(second.onChange).toHaveBeenCalledWith('w-gamma')
  })

  it('Esc closes the popover and never reaches a page-level keydown listener', async () => {
    const pageSpy = vi.fn()
    const winSpy = vi.fn()
    document.addEventListener('keydown', pageSpy)
    window.addEventListener('keydown', winSpy)
    try {
      const { input } = await openDropdown({})
      fireEvent.keyDown(input, { key: 'Escape' })
      expect(screen.queryByPlaceholderText('Search workspaces...')).toBeNull()
      expect(pageSpy).not.toHaveBeenCalled()
      expect(winSpy).not.toHaveBeenCalled()

      // Esc with focus outside the search (the capture-phase listener).
      fireEvent.click(screen.getByTitle('Filter by workspace or project'))
      expect(screen.getByPlaceholderText('Search workspaces...')).toBeTruthy()
      fireEvent.keyDown(document.body, { key: 'Escape' })
      expect(screen.queryByPlaceholderText('Search workspaces...')).toBeNull()
      expect(pageSpy).not.toHaveBeenCalled()
    } finally {
      document.removeEventListener('keydown', pageSpy)
      window.removeEventListener('keydown', winSpy)
    }
  })

  it('an outside mousedown closes it', async () => {
    await openDropdown({})
    fireEvent.mouseDown(document.body)
    expect(screen.queryByPlaceholderText('Search workspaces...')).toBeNull()
  })

  it('Unlinked, Projects, All and workspace rows pick their values', async () => {
    const picks: string[] = []
    for (const v of [UNLINKED_FILTER_VALUE, 'project:pg1', 'all', 'w-beta']) {
      const { onChange } = await openDropdown({ value: 'w-alpha', onChange: (x) => picks.push(x) })
      fireEvent.click(rowFor(v))
      expect(onChange).toHaveBeenCalledTimes(1)
      expect(screen.queryByPlaceholderText('Search workspaces...')).toBeNull()
      cleanup()
    }
    expect(picks).toEqual([UNLINKED_FILTER_VALUE, 'project:pg1', 'all', 'w-beta'])
  })

  it('without showUnlinked there is no Unlinked row', async () => {
    await openDropdown({ showUnlinked: false })
    expect(rowValues()).toEqual(['all', 'project:pg1', 'w-alpha', 'w-beta', 'w-gamma'])
  })

  it('workspace rows look up a missing icon on the connected server; a given icon paints', async () => {
    // Fresh paths: ProjectAvatar caches lookups by path for the module.
    const fresh: FilterableWorkspace[] = [
      { id: 'i1', name: 'Icon', path: '/icon/one', color: '#111111', iconUrl: PNG, focusGroupId: null },
      { id: 'i2', name: 'NoIcon', path: '/icon/two', color: '#222222', iconUrl: null, focusGroupId: null },
    ]
    render(<WorkspaceFilterDropdown projects={fresh} value="all" onChange={() => {}} />)
    fireEvent.click(screen.getByTitle('Filter by workspace or project'))
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0))
    })
    const args = cli.daemonCliGet.mock.calls.map((c) => (c as unknown[]).slice(1))
    expect(args).toEqual([['projects/get-icon', { path: '/icon/two', project_id: 'i2' }]])
    expect(rowFor('i1').querySelector('img')?.getAttribute('src')).toBe(PNG)
    expect(rowFor('i2').querySelector('img')).toBeNull()
  })

  it('the keyboard row scrolls into view', async () => {
    const { input } = await openDropdown({})
    scrolled.mockClear()
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    expect(scrolled).toHaveBeenCalledTimes(1)
    expect(scrolled.mock.contexts[0]).toBe(rowFor('all'))
  })

  it('the trigger shows the current pick', async () => {
    render(<WorkspaceFilterDropdown projects={projects} value="w-beta" onChange={() => {}} />)
    expect(screen.getByTitle('Filter by workspace or project').textContent).toBe('BBeta')
    cleanup()
    render(<WorkspaceFilterDropdown projects={projects} value="project:pg1" onChange={() => {}} />)
    expect(screen.getByTitle('Filter by workspace or project').textContent).toBe('Launch')
    cleanup()
    render(<WorkspaceFilterDropdown projects={projects} value={UNLINKED_FILTER_VALUE} onChange={() => {}} />)
    expect(screen.getByTitle('Filter by workspace or project').textContent).toBe('Unlinked workspace')
  })

  it('looks the same: popover, search, and one row of each kind (groups off)', async () => {
    const { input, popover } = await openDropdown({ value: 'w-alpha' })
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    expect(popover.getAttribute('class')).toMatchInlineSnapshot(`"absolute right-0 top-full mt-1 w-64 z-30 bg-[var(--color-bg)] border border-[var(--color-border)] shadow-lg flex flex-col"`)
    expect(classTree(input.parentElement as Element)).toMatchInlineSnapshot(`
      "div .p-1.5 border-b border-[var(--color-border)]
        input .w-full px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)]"
    `)
    const scroller = popover.querySelector('.overflow-y-auto')
    if (!scroller) throw new Error('no scroll body')
    expect(scroller.getAttribute('class')).toMatchInlineSnapshot(`"max-h-72 overflow-y-auto py-1"`)
    // All (plain), Unlinked, Projects (keyboard row), selected workspace,
    // and a plain workspace with the letter.
    expect(classTree(rowFor('all'))).toMatchInlineSnapshot(`
      "button .flex items-center gap-2 px-2 py-1.5 cursor-pointer transition-colors w-full text-left text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]
        span .flex-shrink-0 w-5 h-5 flex items-center justify-center border border-[var(--color-border)] text-[var(--color-text-muted)]
          svg .
            rect .
            rect .
            rect .
            rect .
        span .text-xs truncate flex-1"
    `)
    expect(classTree(rowFor(UNLINKED_FILTER_VALUE))).toMatchInlineSnapshot(`
      "button .flex items-center gap-2 px-2 py-1.5 cursor-pointer transition-colors w-full text-left text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]
        span .flex-shrink-0 w-5 h-5 flex items-center justify-center border border-dashed border-[var(--color-border)] text-[var(--color-text-muted)]
        span .text-xs truncate flex-1 italic"
    `)
    expect(classTree(rowFor('project:pg1'))).toMatchInlineSnapshot(`
      "button .flex items-center gap-2 px-2 py-1.5 cursor-pointer transition-colors w-full text-left bg-white/[0.06] text-[var(--color-text-primary)]
        span .flex-shrink-0 flex items-center justify-center border border-[var(--color-accent)]/40 text-[var(--color-accent)] {width: 20px; height: 20px;}
          svg .
            polygon .
            polyline .
            polyline .
        span .text-xs truncate flex-1
        span .text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0"
    `)
    expect(classTree(rowFor('w-alpha'))).toMatchInlineSnapshot(`
      "button .flex items-center gap-2 px-2 py-1.5 cursor-pointer transition-colors w-full text-left bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]
        span .flex-shrink-0 {width: 20px; height: 20px; border: 2px solid rgb(224, 108, 117); overflow: hidden; display: block;}
          img . {width: 100%; height: 100%; object-fit: cover; object-position: center; display: block;}
        span .text-xs truncate flex-1"
    `)
    expect(classTree(rowFor('w-beta'))).toMatchInlineSnapshot(`
      "button .flex items-center gap-2 px-2 py-1.5 cursor-pointer transition-colors w-full text-left text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]
        span .flex-shrink-0 {width: 20px; height: 20px;}
          span .flex items-center justify-center {width: 20px; height: 20px; background-color: rgb(97, 175, 239); color: var(--color-on-accent); font-size: 10px; font-weight: 700; line-height: 1; font-family: inherit; display: flex;}
        span .text-xs truncate flex-1"
    `)
    // The Projects header.
    const projectsHeader = screen.getByText('Projects').parentElement
    if (!projectsHeader) throw new Error('no Projects header')
    expect(classTree(projectsHeader)).toMatchInlineSnapshot(`
      "div .flex items-center gap-1.5 px-2 pt-2 pb-1 select-none
        span .w-1 h-3 flex-shrink-0 bg-[var(--color-accent)]
        span .text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] flex-1 truncate
        span .text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0"
    `)
  })

  it('looks the same: focus-group section headers (groups on)', async () => {
    useFocusGroupsStore.setState({
      focusGroupsEnabled: true,
      focusGroups: [{ id: 'g1', name: 'Core', color: '#ff8800', tabOrder: 0, createdAt: 0 }],
    })
    await openDropdown({})
    expect(rowValues()).toEqual(['all', UNLINKED_FILTER_VALUE, 'project:pg1', 'w-alpha', 'w-gamma', 'w-beta'])
    const core = screen.getByText('Core').parentElement
    const ungrouped = screen.getByText('Ungrouped').parentElement
    if (!core || !ungrouped) throw new Error('no group headers')
    expect(classTree(core)).toMatchInlineSnapshot(`
      "div .flex items-center gap-1.5 px-2 pt-2 pb-1 select-none
        span .w-1 h-3 flex-shrink-0 {background-color: rgb(255, 136, 0);}
        span .text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] flex-1 truncate
        span .text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0"
    `)
    expect(classTree(ungrouped)).toMatchInlineSnapshot(`
      "div .flex items-center gap-1.5 px-2 pt-2 pb-1 select-none
        span .text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] flex-1 truncate
        span .text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0"
    `)
    expect(core.textContent).toBe('Core2')
    expect(ungrouped.textContent).toBe('Ungrouped1')
  })
})
