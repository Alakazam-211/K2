// @vitest-environment jsdom
//
// T2.2 (prd-home-picker-and-remote-avatars-v1 P4/P7, vs-live P25/P26/P35):
// the shared searchable agent list. Arrow keys skip checked and disabled
// rows and clamp at the ends, Enter and clicks on a taken row do nothing,
// the roles (combobox → listbox → option), the status slot, `matches`, and
// Esc that only stops when the caller asked.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { useState } from 'react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
const cli = vi.hoisted(() => ({ daemonCliGet: vi.fn(async () => ({ found: false, dataUrl: null })) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: cli.daemonCliGet,
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_scope: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import {
  SearchableAgentList,
  matchesAgentRow,
  pickableAgentValues,
  visibleAgentSections,
  type AgentListRow,
  type AgentListSection,
} from './SearchableAgentList'

if (typeof globalThis.CSS === 'undefined') {
  ;(globalThis as { CSS?: unknown }).CSS = { escape: (v: string) => v.replace(/["\\]/g, '\\$&') }
}
Element.prototype.scrollIntoView = vi.fn()

afterEach(() => cleanup())

function row(value: string, state: AgentListRow['state'] = 'pickable', extra: Partial<AgentListRow> = {}): AgentListRow {
  return {
    value,
    label: value.toUpperCase(),
    avatar: { kind: 'workspace', path: `/x/${value}`, name: value, color: '#123456', iconUrl: null, fetchIcon: false },
    state,
    ...extra,
  }
}

const SECTIONS: AgentListSection[] = [
  { key: 's1', label: 'One', rows: [row('a'), row('b', 'checked', { title: 'Already on Home' }), row('c')] },
  { key: 's2', label: 'Two', rows: [row('d', 'disabled'), row('e')] },
]

function Harness(props: {
  sections?: AgentListSection[]
  onPick?: (v: string) => void
  onEscape?: () => void
  matches?: (r: AgentListRow, q: string) => boolean
}): React.JSX.Element {
  const [query, setQuery] = useState('')
  return (
    <SearchableAgentList
      sections={props.sections ?? SECTIONS}
      query={query}
      onQuery={setQuery}
      onPick={props.onPick ?? (() => {})}
      onEscape={props.onEscape}
      matches={props.matches}
      placeholder="Search agents..."
      emptyText="Nothing here"
      ariaLabel="Agents"
    />
  )
}

function search(): HTMLElement {
  return screen.getByRole('combobox')
}

describe('SearchableAgentList (T2.2)', () => {
  it('the input is a combobox controlling the listbox; rows are options', () => {
    render(<Harness />)
    const input = search()
    const listbox = screen.getByRole('listbox', { name: 'Agents' })
    expect(input.getAttribute('aria-controls')).toBe(listbox.id)
    expect(listbox.id.length).toBeGreaterThan(0)
    const options = within(listbox).getAllByRole('option')
    expect(options.map((o) => o.getAttribute('data-ws-filter-value'))).toEqual(['a', 'b', 'c', 'd', 'e'])
    expect(options.map((o) => o.getAttribute('aria-disabled'))).toEqual([null, 'true', null, 'true', null])
    // The taken row paints the check; the disabled one does not.
    expect(options[1].querySelector('[data-row-check]')).not.toBeNull()
    expect(options[1].getAttribute('title')).toBe('Already on Home')
    expect(options[3].querySelector('[data-row-check]')).toBeNull()
    expect(within(listbox).getAllByRole('group').map((g) => g.getAttribute('aria-label'))).toEqual(['One', 'Two'])
  })

  it('arrow keys skip checked and disabled rows and never wrap past the ends', () => {
    const onPick = vi.fn()
    render(<Harness onPick={onPick} />)
    const input = search()
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(onPick).toHaveBeenLastCalledWith('a')
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    expect(input.getAttribute('aria-activedescendant')).toBe(
      screen.getByRole('option', { name: /^C/ }).id,
    )
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(onPick).toHaveBeenLastCalledWith('c')
    for (let i = 0; i < 10; i++) fireEvent.keyDown(input, { key: 'ArrowDown' })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(onPick).toHaveBeenLastCalledWith('e')
    for (let i = 0; i < 10; i++) fireEvent.keyDown(input, { key: 'ArrowUp' })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(onPick).toHaveBeenLastCalledWith('a')
    expect(onPick.mock.calls.map((c) => c[0])).toEqual(['a', 'c', 'e', 'a'])
  })

  it('Enter with no keyboard row and clicks on taken rows do nothing', () => {
    const onPick = vi.fn()
    render(<Harness onPick={onPick} />)
    fireEvent.keyDown(search(), { key: 'Enter' })
    fireEvent.click(screen.getByRole('option', { name: /^B/ }))
    fireEvent.click(screen.getByRole('option', { name: /^D/ }))
    expect(onPick).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('option', { name: /^E/ }))
    expect(onPick).toHaveBeenCalledWith('e')
  })

  it('Enter on a row that turned checked does not pick it', () => {
    const onPick = vi.fn()
    const { rerender } = render(<Harness onPick={onPick} />)
    fireEvent.keyDown(search(), { key: 'ArrowDown' })
    const taken: AgentListSection[] = [{ key: 's1', label: 'One', rows: [row('a', 'checked'), row('b', 'checked')] }]
    rerender(<Harness onPick={onPick} sections={taken} />)
    fireEvent.keyDown(search(), { key: 'Enter' })
    expect(onPick).not.toHaveBeenCalled()
  })

  it('Esc bubbles unless the caller passes onEscape', () => {
    const pageSpy = vi.fn()
    document.addEventListener('keydown', pageSpy)
    try {
      render(<Harness />)
      fireEvent.keyDown(search(), { key: 'Escape' })
      expect(pageSpy).toHaveBeenCalledTimes(1)
      cleanup()

      const onEscape = vi.fn()
      render(<Harness onEscape={onEscape} />)
      fireEvent.keyDown(search(), { key: 'Escape' })
      expect(onEscape).toHaveBeenCalledTimes(1)
      expect(pageSpy).toHaveBeenCalledTimes(1)
    } finally {
      document.removeEventListener('keydown', pageSpy)
    }
  })

  it('matches filters rows; a section with a status stays, one without hides', () => {
    const sections: AgentListSection[] = [
      { key: 'ok', label: 'Ok', rows: [row('alpha', 'pickable', { keywords: ['/srv/alpha', 'al'] }), row('beta')] },
      { key: 'wait', label: 'Waiting', status: <p>Loading agents…</p>, rows: [] },
    ]
    render(<Harness sections={sections} matches={matchesAgentRow} />)
    fireEvent.change(search(), { target: { value: 'srv' } })
    expect(screen.getAllByRole('option').map((o) => o.getAttribute('data-ws-filter-value'))).toEqual(['alpha'])
    expect(screen.getByText('Loading agents…')).toBeTruthy()
    fireEvent.change(search(), { target: { value: 'zzz' } })
    expect(screen.queryAllByRole('option')).toEqual([])
    expect(screen.queryByRole('group', { name: 'Ok' })).toBeNull()
    expect(screen.getByRole('group', { name: 'Waiting' })).toBeTruthy()
    expect(screen.queryByText('Nothing here')).toBeNull()
  })

  it('shows the empty text when no section is left', () => {
    render(<Harness sections={[{ key: 'x', label: 'X', rows: [row('a')] }]} matches={matchesAgentRow} />)
    fireEvent.change(search(), { target: { value: 'nope' } })
    expect(screen.getByText('Nothing here')).toBeTruthy()
  })

  it('pure helpers: visible sections, pickable values, matching', () => {
    expect(pickableAgentValues(SECTIONS)).toEqual(['a', 'c', 'e'])
    expect(visibleAgentSections(SECTIONS, 'b', matchesAgentRow).map((s) => s.rows.map((r) => r.value))).toEqual([['b']])
    // An empty query keeps every section as given.
    const all = visibleAgentSections(SECTIONS, '', matchesAgentRow)
    expect(all.length).toBe(2)
    expect(all[0]).toBe(SECTIONS[0])
    expect(all[1]).toBe(SECTIONS[1])
    expect(matchesAgentRow(row('x', 'pickable', { keywords: ['handle-x'] }), 'HANDLE')).toBe(true)
    expect(matchesAgentRow(row('x'), '  ')).toBe(true)
    expect(matchesAgentRow(row('x'), 'y')).toBe(false)
  })

  it('a node avatar renders as given; a workspace avatar with fetchIcon off asks no server', () => {
    render(
      <Harness
        sections={[
          {
            key: 'n',
            label: null,
            rows: [{ value: 'g', label: 'Glyph', state: 'pickable', avatar: { kind: 'node', node: <i data-testid="glyph" /> } }],
          },
          { key: 'w', label: null, rows: [row('w')] },
        ]}
      />,
    )
    expect(screen.getByTestId('glyph')).toBeTruthy()
    expect(cli.daemonCliGet).not.toHaveBeenCalled()
  })
})
