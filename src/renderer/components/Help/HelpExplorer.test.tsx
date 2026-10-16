// @vitest-environment jsdom
//
// Help (Rosson 2026-10-16): the top-bar Help button opens a CLI tool
// explorer. Rail = CLI tools (k2 with its noun families, k2-node) + pages
// (k2 study, the daemon glossary); right = nouns/commands or a page; top
// middle = one search across all of it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn<(scope: unknown, route: string, params?: Record<string, string>) => Promise<unknown>>(),
}))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: (...a: [unknown, string, Record<string, string>?]) => h.daemonCliGet(...a) }))

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import HelpButton from './HelpButton'
import HelpExplorer from './HelpExplorer'
import catalog from '@shared/cli-catalog.generated.json'
import { buildExplorerModel, searchExplorer, glossaryPages, type CliCatalog } from './cli-explorer'
import type { ServerScope } from '@/kessel/server-scope'

/** The server Help reads its glossary from (the top bar's). */
const SCOPE = { id: 'help-test-scope' } as unknown as ServerScope

const GLOSSARY = [
  { term: 'workspace', summary: 'A folder K2SO manages (has exactly one primary agent)' },
  { term: 'heartbeat', summary: 'Workspace-scoped scheduled wake (cron-like)' },
]

function daemonUp(): void {
  h.daemonCliGet.mockImplementation(async (_s, route, params) => {
    if (route === 'glossary') return GLOSSARY
    if (route === 'glossary/get') return { term: params?.term, summary: 's', definition: `Full text for ${params?.term} in K2SO.` }
    throw new Error(`unexpected route ${route}`)
  })
}

function daemonDown(): void {
  h.daemonCliGet.mockImplementation(async () => {
    throw new Error('daemon not reachable')
  })
}

beforeEach(() => {
  h.daemonCliGet.mockReset()
  daemonUp()
})
afterEach(cleanup)

function search(): HTMLInputElement {
  const el = screen.getByTestId('help-search')
  if (!(el instanceof HTMLInputElement)) throw new Error('search is not an input')
  return el
}

function results(): HTMLElement[] {
  return screen.queryAllByTestId('help-search-result')
}

describe('Help button', () => {
  it('opens the explorer (lazy) and lights up while open', async () => {
    render(<HelpButton />)
    const button = screen.getByRole('button', { name: 'Help' })
    expect(button.getAttribute('aria-pressed')).toBe('false')
    fireEvent.click(button)
    expect(button.getAttribute('aria-pressed')).toBe('true')
    const dialog = await screen.findByRole('dialog', { name: 'Help' })
    expect(within(dialog).getByTestId('help-rail')).toBeTruthy()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(button.getAttribute('aria-pressed')).toBe('false')
  })
})

describe('the rail lists the CLI tools and the pages', () => {
  it('k2 (with its noun families) first, then k2-node; then k2 study and Glossary', () => {
    render(<HelpExplorer scope={SCOPE} onClose={() => {}} />)
    const rail = screen.getByTestId('help-rail')
    const labels = Array.from(rail.querySelectorAll('h3, button')).map((el) => el.textContent?.trim())
    expect(labels).toEqual([
      'CLI tools',
      'k2',
      'Talk',
      'Inbox & humans',
      'Identity',
      'Agents & groups',
      'Always-on & more',
      'Mail & calendars',
      'Apps, Gardens & data',
      'Network & domains',
      'Server',
      'Help & docs',
      'k2-node',
      'Pages',
      'k2 study',
      'Glossary',
    ])
    expect(screen.getByTestId('help-rail-k2').getAttribute('aria-current')).toBe('true')
    // k2 is selected first: every family's nouns.
    expect(screen.getByTestId('help-noun-k2:msg')).toBeTruthy()
    expect(screen.getByTestId('help-noun-k2:vault')).toBeTruthy()
  })

  it('a family shows only its nouns; k2-node shows its commands', () => {
    render(<HelpExplorer scope={SCOPE} onClose={() => {}} />)
    fireEvent.click(screen.getByTestId('help-rail-k2-mail'))
    expect(screen.getByTestId('help-rail-k2-mail').getAttribute('aria-current')).toBe('true')
    expect(screen.getByTestId('help-rail-k2').getAttribute('aria-current')).toBeNull()
    const main = screen.getByTestId('help-main')
    expect(Array.from(main.querySelectorAll('[data-testid^="help-noun-"]')).map((n) => n.getAttribute('data-testid'))).toEqual([
      'help-noun-k2:mail',
      'help-noun-k2:hostmail',
      'help-noun-k2:calendar',
    ])
    expect(within(main).getByText('Hosted addresses for agents (not the inbox tray). Needs the mail sidecar on Linux.')).toBeTruthy()

    fireEvent.click(screen.getByTestId('help-rail-k2-node'))
    expect(screen.getByTestId('help-tool-k2-node')).toBeTruthy()
    expect(screen.getByTestId('help-noun-k2-node:enroll')).toBeTruthy()
    expect(screen.getByText(/Agents never run this/)).toBeTruthy()
  })

  it('a noun expands to its commands from k2 --schema, with copy buttons', async () => {
    const writeText = vi.fn(async (_t: string) => undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    render(<HelpExplorer scope={SCOPE} onClose={() => {}} />)
    const card = screen.getByTestId('help-noun-k2:agent')
    expect(card.getAttribute('data-expanded')).toBe('false')
    fireEvent.click(within(card).getByRole('button', { expanded: false }))
    expect(card.getAttribute('data-expanded')).toBe('true')
    const verbs = within(card).getAllByTestId('help-verb').map((v) => v.getAttribute('data-verb'))
    const schema = (catalog as CliCatalog).k2.commands.filter((c) => c.name.split(' ')[0] === 'agent').map((c) => `k2 ${c.name}`)
    expect(verbs).toEqual(schema)
    expect(verbs).toContain('k2 agent hire')
    fireEvent.click(within(screen.getByTestId('help-example-k2:agent')).getByTestId('help-copy'))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('k2 agent hire ~/ai/sales --name sales'))
    expect(await within(screen.getByTestId('help-example-k2:agent')).findByText('Copied')).toBeTruthy()
  })

  it('a noun --schema does not list shows its captured --help, or points at k2 help', () => {
    render(<HelpExplorer scope={SCOPE} onClose={() => {}} />)
    const compute = screen.getByTestId('help-noun-k2:compute')
    fireEvent.click(within(compute).getByRole('button', { expanded: false }))
    expect(within(compute).getByText(/run commands on your own machines/)).toBeTruthy()
    const vault = screen.getByTestId('help-noun-k2:vault')
    expect(within(vault).getByText('k2 help vault')).toBeTruthy()
  })

  it('k2 study lists the pages and opens one', () => {
    render(<HelpExplorer scope={SCOPE} onClose={() => {}} />)
    fireEvent.click(screen.getByTestId('help-rail-study'))
    expect(screen.getByTestId('help-pages-study')).toBeTruthy()
    fireEvent.click(screen.getByTestId('help-page-link-study:errors'))
    const page = screen.getByTestId('help-page-study:errors')
    expect(within(page).getByText('Exit codes')).toBeTruthy()
    expect(within(page).getByTestId('help-page-body').textContent).toContain('k2 study errors')
    fireEvent.click(within(page).getByText(/All pages/))
    expect(screen.getByTestId('help-pages-study')).toBeTruthy()
  })
})

describe('glossary: from the daemon, with a fallback', () => {
  it('lists the daemon terms (shown as K2) and loads a definition on open', async () => {
    render(<HelpExplorer scope={SCOPE} onClose={() => {}} />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith(SCOPE, 'glossary'))
    fireEvent.click(screen.getByTestId('help-rail-glossary'))
    const list = await screen.findByTestId('help-pages-glossary')
    expect(within(list).getByText('A folder K2 manages (has exactly one primary agent)')).toBeTruthy()
    fireEvent.click(within(list).getByTestId('help-page-link-glossary:workspace'))
    const body = await screen.findByTestId('help-page-body')
    expect(body.textContent).toBe('Full text for workspace in K2.')
    expect(h.daemonCliGet).toHaveBeenCalledWith(SCOPE, 'glossary/get', { term: 'workspace' })
  })

  it('daemon down: Glossary says so, everything else still works (search included)', async () => {
    daemonDown()
    render(<HelpExplorer scope={SCOPE} onClose={() => {}} />)
    await waitFor(() => expect(screen.getByTestId('help-rail-glossary').textContent).toContain('offline'))
    fireEvent.click(screen.getByTestId('help-rail-glossary'))
    expect(screen.getByTestId('help-glossary-unavailable').textContent).toContain('did not answer')
    fireEvent.change(search(), { target: { value: 'thread' } })
    expect(results().length).toBeGreaterThan(0)
    expect(results().some((r) => r.getAttribute('data-kind') === 'term')).toBe(false)
  })
})

describe('search across tools, nouns, commands and pages', () => {
  it('finds nouns, commands, study pages and glossary terms, with type badges', async () => {
    render(<HelpExplorer scope={SCOPE} onClose={() => {}} />)
    await waitFor(() => expect(screen.getByTestId('help-rail-glossary').textContent).toBe('Glossary'))
    fireEvent.change(search(), { target: { value: 'heartbeat' } })
    const kinds = new Set(results().map((r) => r.getAttribute('data-kind')))
    expect([...kinds].sort()).toEqual(['command', 'noun', 'page', 'term'])
    expect(results()[0].textContent).toContain('Noun')
    expect(results()[0].textContent).toContain('k2 heartbeat')
    fireEvent.change(search(), { target: { value: 'exit codes' } })
    const page = results().find((r) => r.getAttribute('data-kind') === 'page')
    expect(page?.textContent).toContain('Page')
    expect(page?.textContent).toContain('Exit codes')
  })

  it('click jumps to that tool + noun (family selected, noun expanded, command highlighted)', () => {
    render(<HelpExplorer scope={SCOPE} onClose={() => {}} />)
    fireEvent.change(search(), { target: { value: 'agent hire' } })
    const hit = results().find((r) => r.getAttribute('data-kind') === 'command' && r.textContent?.includes('k2 agent hire'))
    if (!hit) throw new Error('no k2 agent hire result')
    fireEvent.mouseDown(hit)
    expect(search().value).toBe('')
    expect(results()).toHaveLength(0)
    expect(screen.getByTestId('help-rail-k2-agents').getAttribute('aria-current')).toBe('true')
    const card = screen.getByTestId('help-noun-k2:agent')
    expect(card.getAttribute('data-expanded')).toBe('true')
    expect(card.querySelector('[data-highlight="true"]')?.getAttribute('data-verb')).toBe('k2 agent hire')
  })

  it('↑/↓ move the active result, Enter opens it (a page)', () => {
    render(<HelpExplorer scope={SCOPE} onClose={() => {}} />)
    const input = search()
    fireEvent.change(input, { target: { value: 'people' } })
    const all = results()
    expect(all.length).toBeGreaterThan(1)
    expect(all[0].getAttribute('aria-selected')).toBe('true')
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    expect(results()[1].getAttribute('aria-selected')).toBe('true')
    expect(input.getAttribute('aria-activedescendant')).toBe(results()[1].id)
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    expect(results()[0].getAttribute('aria-selected')).toBe('true')
    const pageIndex = results().findIndex((r) => r.getAttribute('data-kind') === 'page' && r.textContent?.includes('Four people lists'))
    expect(pageIndex).toBeGreaterThanOrEqual(0)
    for (let i = 0; i < pageIndex; i++) fireEvent.keyDown(input, { key: 'ArrowDown' })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(screen.getByTestId('help-page-study:people')).toBeTruthy()
    expect(screen.getByTestId('help-rail-study').getAttribute('aria-current')).toBe('true')
  })

  it('no match says so', () => {
    render(<HelpExplorer scope={SCOPE} onClose={() => {}} />)
    fireEvent.change(search(), { target: { value: 'zzqqxx' } })
    expect(screen.getByTestId('help-search-empty').textContent).toContain('zzqqxx')
  })

  it('⌘K / Ctrl+K focus the search; Esc clears it first, then closes', () => {
    const onClose = vi.fn()
    render(<HelpExplorer scope={SCOPE} onClose={onClose} />)
    act(() => screen.getByTestId('help-rail-study').focus())
    expect(document.activeElement).not.toBe(search())
    fireEvent.keyDown(window, { key: 'k', metaKey: true })
    expect(document.activeElement).toBe(search())
    act(() => screen.getByTestId('help-rail-study').focus())
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    expect(document.activeElement).toBe(search())
    fireEvent.change(search(), { target: { value: 'msg' } })
    fireEvent.keyDown(search(), { key: 'Escape' })
    expect(search().value).toBe('')
    expect(onClose).not.toHaveBeenCalled()
    fireEvent.keyDown(search(), { key: 'Escape' })
    expect(onClose).toHaveBeenCalledTimes(1)
  })
})

describe('searchExplorer (model)', () => {
  const model = buildExplorerModel(catalog as CliCatalog)

  it('`k2 msg` and `msg` both rank the msg noun first', () => {
    for (const q of ['msg', 'k2 msg']) {
      const top = searchExplorer(model, [], q)[0]
      expect([q, top.kind, top.label]).toEqual([q, 'noun', 'k2 msg'])
    }
  })

  it('aliases find their noun (tickets → feedback, cal → calendar)', () => {
    expect(searchExplorer(model, [], 'tickets').find((r) => r.kind === 'noun')?.label).toBe('k2 feedback')
    expect(searchExplorer(model, [], 'cal').some((r) => r.kind === 'noun' && r.label === 'k2 calendar')).toBe(true)
  })

  it('k2-node commands and the tool itself are searchable', () => {
    const r = searchExplorer(model, [], 'k2-node')
    expect(r[0]).toMatchObject({ kind: 'tool', label: 'k2-node' })
    expect(searchExplorer(model, [], 'drain').some((x) => x.label === 'k2-node drain')).toBe(true)
  })

  it('glossary terms join the results as Term', () => {
    const r = searchExplorer(model, glossaryPages(GLOSSARY), 'cron-like')
    expect(r.map((x) => [x.kind, x.label])).toContainEqual(['term', 'heartbeat'])
  })

  it('an empty query has no results', () => {
    expect(searchExplorer(model, [], '   ')).toEqual([])
  })
})
