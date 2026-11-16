// @vitest-environment jsdom

// Settings → Cores → Multiplayer (Beta), prd-multiplayer-cores-settings-v1
// §1, §10.4. The rows come from the daemon's `GET /cli/docs/cores`; a click
// posts exactly `{"multiplayerCores": {"<id>": <bool>}}` to settings/update;
// turning a core off while files are open live asks first; a Member sees the
// switches disabled; the daemon's refusal shows under the row; a
// `doc_cores_changed` refetches.

import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

type Json = Record<string, unknown>

const h = vi.hoisted(() => ({
  cores: null as unknown,
  gets: [] as string[],
  posts: [] as Array<{ route: string; body: unknown }>,
  postError: null as Error | null,
  getError: null as Error | null,
  postGate: null as Promise<void> | null,
}))

const events = vi.hoisted(() => ({ handlers: new Set<(e: unknown) => void>() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

vi.mock('@/lib/daemon-cli', async (orig) => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    ...(await orig<typeof import('@/lib/daemon-cli')>()),
    daemonCliGet: primaryOnly(async (route: string) => {
      if (route !== 'docs/cores') throw new Error(`unexpected GET ${route}`)
      h.gets.push(route)
      if (h.getError) throw h.getError
      return h.cores
    }),
    daemonCliPost: primaryOnly(async (route: string, body: unknown) => {
      h.posts.push({ route, body })
      if (h.postGate) await h.postGate
      if (h.postError) throw h.postError
      if (route !== 'settings/update') throw new Error(`unexpected POST ${route}`)
      return { multiplayerCoresReport: { changed: [], closed: 0, written: 0, failed: [] } }
    }),
  }
})

vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onDocCoresChanged: (_scope: unknown, fn: (e: unknown) => void) => {
    events.handlers.add(fn)
    return () => void events.handlers.delete(fn)
  },
}))

import ConfirmDialog from '@/components/ConfirmDialog/ConfirmDialog'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { BOARD_FILE_PREVIEW_LABEL, MultiplayerSection, MULTIPLAYER_INTRO, MULTIPLAYER_MANIFEST, MULTIPLAYER_MEMBER_LINE } from './MultiplayerSection'
import { useSettingsStore } from '@/stores/settings'
import { failedWriteBackLine, openNowLine, parseDocCores, plainCore, turnOffMessage, turnOffTitle } from './multiplayer-cores-api'
import { searchManifest, SECTION_LABELS } from '../searchManifest'

/** The page's own switches: the live workbook preview's (a per-computer key, its own tests) is left out. */
const coreSwitches = (): HTMLElement[] => screen.getAllByRole('switch').filter((s) => !s.closest('[data-testid="multiplayer-live-workbook"], [data-testid="editor-preview-switches"]'))

function core(id: string, name: string, over: Json = {}): Json {
  return {
    id,
    name,
    status: 'beta',
    description: `${name} description.`,
    view: id === 'any-file' ? 'none' : 'live',
    on: false,
    open: { files: 0, participants: 0, paths: [] },
    lastChange: null,
    ...over,
  }
}

function answer(over: Json = {}, markdown: Json = {}): Json {
  return {
    ok: true,
    canChange: true,
    later: ['CSV grid', 'Word'],
    cores: [
      core('markdown', 'Markdown core', markdown),
      core('text', 'Text and code core'),
      core('any-file', 'Any other file'),
    ],
    ...over,
  }
}

function sw(name: string): HTMLButtonElement {
  return screen.getByRole('switch', { name }) as HTMLButtonElement
}

function row(id: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-settings-id="multiplayer.${id}"]`)
  if (!el) throw new Error(`no row multiplayer.${id}`)
  return el
}

async function mount(): Promise<void> {
  render(
    <>
      <MultiplayerSection />
      <ConfirmDialog />
    </>,
  )
  await waitFor(() => expect(coreSwitches()).toHaveLength(3))
}

async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0))
  })
}

beforeEach(() => {
  h.cores = answer()
  h.gets = []
  h.posts = []
  h.postError = null
  h.getError = null
  h.postGate = null
  events.handlers.clear()
})

afterEach(() => {
  cleanup()
})

describe('Settings → Cores → Multiplayer', () => {
  it('renders one row per core from GET /cli/docs/cores, with the intro, pills and Coming later', async () => {
    h.cores = answer({
      later: ['CSV grid', 'Word', 'Slides'],
      cores: [core('markdown', 'Markdown core'), core('text', 'Text and code core'), core('extra', 'Extra core', { status: 'stable' })],
    })
    await mount()
    expect(h.gets).toEqual(['docs/cores'])
    expect(screen.getByRole('heading', { name: 'Multiplayer' })).toBeTruthy()
    expect(screen.getByText(MULTIPLAYER_INTRO)).toBeTruthy()
    expect(MULTIPLAYER_INTRO).toBe(
      "People and agents work in the same file at the same time, with presence, history and everyone's own undo. Choose which kinds of files open this way on this server. It applies to everyone here; the rest open the usual way.",
    )
    // The rows are the daemon's, not a hard-coded list.
    // Known cores read as plain kinds of file; one this client doesn't know
    // keeps the daemon's name.
    expect(coreSwitches().map((s) => s.getAttribute('aria-label'))).toEqual([
      'Markdown files',
      'Text and code files',
      'Extra core',
    ])
    expect(sw('Markdown files').getAttribute('aria-checked')).toBe('false')
    expect(sw('Markdown files').disabled).toBe(false)
    expect(within(row('markdown')).getByText('Beta')).toBeTruthy()
    expect(within(row('markdown')).getByText('.md and .markdown files open beside the workspace’s agent, with live editing, presence and history.')).toBeTruthy()
    expect(within(row('extra')).getByText('Extra core description.')).toBeTruthy()
    expect(row('markdown').textContent).not.toContain('core')
    expect(within(row('extra')).queryByText('Beta')).toBeNull()
    expect(screen.queryByText(MULTIPLAYER_MEMBER_LINE)).toBeNull()
    expect(screen.getByTestId('multiplayer-later').textContent).toBe('Coming later: CSV grid, Word, Slides.')
    // Off: no beta line, no open-now line.
    expect(screen.queryByText('Keep your work in git while multiplayer is new.')).toBeNull()
  })

  it('Member: switches visible but disabled, plus the line; a click writes nothing', async () => {
    h.cores = answer({ canChange: false })
    await mount()
    for (const s of coreSwitches()) expect((s as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText("Only the server's owner or an Admin can change these.")).toBeTruthy()
    fireEvent.click(sw('Markdown files'))
    await settle()
    expect(h.posts).toEqual([])
  })

  it('turning on: no dialog, the write body is exact, then a refetch', async () => {
    await mount()
    h.cores = answer({}, { on: true })
    fireEvent.click(sw('Markdown files'))
    await waitFor(() => expect(sw('Markdown files').getAttribute('aria-checked')).toBe('true'))
    expect(screen.queryByText('Turn off multiplayer for Markdown files?')).toBeNull()
    expect(within(row('markdown')).getByRole('status').textContent).toBe('Multiplayer is on for Markdown files, for everyone on this server.')
    expect(h.posts).toEqual([{ route: 'settings/update', body: { multiplayerCores: { markdown: true } } }])
    expect(JSON.stringify(h.posts[0].body)).toBe('{"multiplayerCores":{"markdown":true}}')
    expect(h.gets).toEqual(['docs/cores', 'docs/cores'])
    // While on: the beta line under the switch.
    expect(within(row('markdown')).getByText('Keep your work in git while multiplayer is new.')).toBeTruthy()
    expect(within(row('text')).queryByText('Keep your work in git while multiplayer is new.')).toBeNull()
  })

  it('turning off with nothing open flips with no dialog', async () => {
    h.cores = answer({}, { on: true })
    await mount()
    h.cores = answer()
    fireEvent.click(sw('Markdown files'))
    await waitFor(() => expect(h.posts).toHaveLength(1))
    expect(screen.queryByText('Turn off multiplayer for Markdown files?')).toBeNull()
    expect(h.posts[0]).toEqual({ route: 'settings/update', body: { multiplayerCores: { markdown: false } } })
  })

  it('turning off with files open live asks first: Cancel writes nothing, Turn off writes {markdown:false}', async () => {
    const open = { files: 2, participants: 3, paths: ['notes.md', 'plan.md'] }
    h.cores = answer({}, { on: true, open })
    await mount()
    fireEvent.click(sw('Markdown files'))
    await waitFor(() => expect(screen.getByText('Turn off multiplayer for Markdown files?')).toBeTruthy())
    expect(
      screen.getByText(
        '2 files are open live (notes.md, plan.md). K2 writes every change to disk, closes them, and they reopen the usual way. Their history stays.',
      ),
    ).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await settle()
    expect(screen.queryByText('Turn off multiplayer for Markdown files?')).toBeNull()
    expect(h.posts).toEqual([])
    expect(sw('Markdown files').getAttribute('aria-checked')).toBe('true')

    fireEvent.click(sw('Markdown files'))
    await waitFor(() => expect(screen.getByText('Turn off multiplayer for Markdown files?')).toBeTruthy())
    h.cores = answer()
    fireEvent.click(screen.getByRole('button', { name: 'Turn off' }))
    await waitFor(() => expect(sw('Markdown files').getAttribute('aria-checked')).toBe('false'))
    expect(h.posts).toEqual([{ route: 'settings/update', body: { multiplayerCores: { markdown: false } } }])
  })

  it("says what failed under the row, with the daemon's refusal quietly under it (role=alert)", async () => {
    await mount()
    h.postError = new DaemonHttpError('updating "multiplayerCores" requires the Owner or Admin role', 403, null)
    fireEvent.click(sw('Markdown files'))
    const alert = await within(row('markdown')).findByRole('alert')
    expect(alert.textContent).toBe(
      'Couldn’t turn on multiplayer for Markdown files. It’s still off.The server said: updating "multiplayerCores" requires the Owner or Admin role.',
    )
    expect(within(row('text')).queryByRole('alert')).toBeNull()
    expect(sw('Markdown files').getAttribute('aria-checked')).toBe('false')
  })

  it('a doc_cores_changed event refetches and repaints', async () => {
    await mount()
    expect(h.gets).toEqual(['docs/cores'])
    expect(events.handlers.size).toBe(1)
    h.cores = answer({}, { on: true })
    act(() => {
      for (const fn of events.handlers) {
        fn({ kind: 'doc_cores_changed', cores: { markdown: true, text: false, 'any-file': false }, core: 'markdown', on: true, by: 'Anna' })
      }
    })
    await waitFor(() => expect(sw('Markdown files').getAttribute('aria-checked')).toBe('true'))
    expect(h.gets).toEqual(['docs/cores', 'docs/cores'])
  })

  it('the open-now line shows only while the core is on and files are open', async () => {
    h.cores = answer({}, { on: true, open: { files: 2, participants: 3, paths: ['notes.md', 'plan.md'] } })
    await mount()
    expect(screen.getByTestId('multiplayer-open-markdown').textContent).toBe(
      'Open live now: 2 files (notes.md, plan.md), 3 people and agents.',
    )
    cleanup()
    // Off (the runtime still counting): no open-now line.
    h.cores = answer({}, { on: false, open: { files: 2, participants: 3, paths: ['notes.md', 'plan.md'] } })
    await mount()
    expect(screen.queryByTestId('multiplayer-open-markdown')).toBeNull()
    // Singular wording.
    expect(openNowLine({ files: 1, participants: 1, paths: ['notes.md'] })).toBe(
      'Open live now: 1 file (notes.md), 1 person or agent.',
    )
    expect(turnOffMessage({ files: 1, participants: 1, paths: ['notes.md'] })).toBe(
      '1 file is open live (notes.md). K2 writes every change to disk, closes them, and they reopen the usual way. Their history stays.',
    )
  })

  it('a failed write-back from the last turn-off shows one line per path until the core is on again', async () => {
    const lastChange = {
      on: false,
      by: 'user:anna',
      at: 1761830000,
      closed: 2,
      written: 1,
      failed: [
        { path: 'notes.md', error: 'no space left on the disk' },
        { path: 'docs/plan.md', error: 'permission denied' },
      ],
    }
    h.cores = answer({}, { on: false, lastChange })
    await mount()
    expect(screen.getAllByTestId('multiplayer-failed-markdown').map((e) => e.textContent)).toEqual([
      "notes.md couldn't be written when multiplayer was turned off for Markdown files (no space left on the disk). Its last changes are kept in K2's history, and turning multiplayer on again for Markdown files writes them.",
      "docs/plan.md couldn't be written when multiplayer was turned off for Markdown files (permission denied). Its last changes are kept in K2's history, and turning multiplayer on again for Markdown files writes them.",
    ])
    cleanup()
    h.cores = answer({}, { on: true, lastChange: { ...lastChange, on: false } })
    await mount()
    expect(screen.queryByTestId('multiplayer-failed-markdown')).toBeNull()
  })

  it('a malformed answer is a plain error card with Try again, never a guess or a crash', async () => {
    expect(() => parseDocCores({ ok: true, canChange: true, cores: [{ id: 'markdown' }] })).toThrow('cores[0].name is not text')
    expect(() => parseDocCores({ ok: true, cores: [] })).toThrow("the reply's canChange is not true or false")
    expect(() => parseDocCores({})).toThrow("the server's answer was missing the list of file kinds")
    h.cores = {}
    render(<MultiplayerSection />)
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('Couldn’t load the multiplayer settings')
    expect(alert.textContent).toContain("The server's answer was missing the list of file kinds.")
    expect(alert.textContent).not.toContain('docs/cores')
    h.cores = answer()
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(coreSwitches()).toHaveLength(3))
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('a refused read says so in words, with the server’s reason quietly', async () => {
    h.getError = new DaemonHttpError('docs runtime is starting', 503, null)
    render(<MultiplayerSection />)
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('Couldn’t load the multiplayer settings')
    expect(alert.textContent).toContain('The server said: docs runtime is starting.')
  })

  it('a server with no cores says so instead of an empty card', async () => {
    h.cores = answer({ cores: [] })
    render(<MultiplayerSection />)
    expect((await screen.findByTestId('multiplayer-empty')).textContent).toContain('No kinds of files to choose yet')
  })

  it('search finds "Multiplayer" by name and keywords', () => {
    expect(SECTION_LABELS.multiplayer).toBe('Multiplayer')
    expect(MULTIPLAYER_MANIFEST.map((e) => [e.id, e.section, e.label])).toEqual([
      ['multiplayer.cores', 'multiplayer', 'Multiplayer'],
      ['multiplayer.new-markdown-editor', 'multiplayer', 'New markdown editor'],
      ['multiplayer.new-code-editor', 'multiplayer', 'New code editor'],
    ])
    expect(MULTIPLAYER_MANIFEST[0].keywords).toEqual(['live docs', 'live editing', 'multiplayer', 'Markdown', 'cores', 'co-editing', 'Board', 'k5board', 'whiteboard', 'PSD', 'CAD', '3D', 'blend', 'DWG', 'STEP', 'STL', 'any other file', 'file lock', 'versions'])
    // Search finds the any-file core by file type, and names no product.
    for (const kind of ['PSD', 'CAD', '3D', 'blend', 'DWG']) {
      expect(searchManifest(MULTIPLAYER_MANIFEST, kind).map((e) => e.id)).toEqual(['multiplayer.cores'])
    }
    expect(MULTIPLAYER_MANIFEST[0].description).toContain('PSD, CAD, 3D and .blend files')
    expect(JSON.stringify(MULTIPLAYER_MANIFEST)).not.toMatch(/photoshop|adobe|blender/i)
    for (const q of ['Multiplayer', 'live docs', 'live editing', 'co-editing', 'board', 'k5board']) {
      expect(searchManifest(MULTIPLAYER_MANIFEST, q).map((e) => e.id)).toEqual(['multiplayer.cores'])
    }
    // "markdown" also finds the new markdown editor's switch, which lives here too.
    expect(searchManifest(MULTIPLAYER_MANIFEST, 'markdown').map((e) => e.id).sort()).toEqual(['multiplayer.cores', 'multiplayer.new-markdown-editor'])
  })
})

describe('core names in a sentence', () => {
  it('names a known core as its kind of file, and an unknown one by the daemon’s name', () => {
    expect(turnOffTitle({ id: 'markdown', name: 'Markdown core', description: '' })).toBe('Turn off multiplayer for Markdown files?')
    expect(turnOffTitle({ id: 'any-file', name: 'Any other file', description: '' })).toBe('Turn off multiplayer for any other file?')
    expect(turnOffTitle({ id: 'sheets', name: 'Sheets core', description: '' })).toBe('Turn off multiplayer for Sheets core?')
    expect(failedWriteBackLine({ id: 'text', name: 'Text and code core', description: '' }, { path: 'a.txt', error: 'no space left on the disk' })).toBe(
      "a.txt couldn't be written when multiplayer was turned off for text and code files (no space left on the disk). " +
        "Its last changes are kept in K2's history, and turning multiplayer on again for text and code files writes them.",
    )
    // The Board's daemon text names a CLI call; Settings says it plainly.
    expect(plainCore({ id: 'board', name: 'Board', description: "Off: agents' k2 board calls are refused" })).toEqual({
      name: 'Boards',
      description: 'A whiteboard Garden for you and your agents, single-person for now. When this is off, boards are hidden and agents can’t open them. Your boards are kept.',
      sentence: 'boards',
    })
  })
})

describe('Settings → Cores → Multiplayer: while a switch saves', () => {
  it('shows Saving… beside the switch and holds it until the write lands', async () => {
    await mount()
    let release: () => void = () => {}
    h.postGate = new Promise<void>((r) => (release = r))
    fireEvent.click(sw('Markdown files'))
    expect((await screen.findByTestId('multiplayer-saving')).textContent).toBe('Saving…')
    expect(sw('Markdown files').disabled).toBe(true)
    h.cores = answer({}, { on: true })
    release()
    await waitFor(() => expect(screen.queryByTestId('multiplayer-saving')).toBeNull())
    expect(sw('Markdown files').getAttribute('aria-checked')).toBe('true')
  })
})

describe('Settings → Cores → Multiplayer: Board files in a tab', () => {
  async function mountWith(n: number): Promise<void> {
    render(<MultiplayerSection />)
    await waitFor(() => expect(coreSwitches()).toHaveLength(n))
  }

  it('shows the switch with the server value, posts the setting, and reads it back', async () => {
    h.cores = answer({ boardFilePreview: false })
    await mountWith(4)
    expect(sw(BOARD_FILE_PREVIEW_LABEL).getAttribute('aria-checked')).toBe('false')
    h.cores = answer({ boardFilePreview: true })
    fireEvent.click(sw(BOARD_FILE_PREVIEW_LABEL))
    await waitFor(() => expect(sw(BOARD_FILE_PREVIEW_LABEL).getAttribute('aria-checked')).toBe('true'))
    expect(h.posts).toEqual([{ route: 'settings/update', body: { boardFilePreview: true } }])
  })

  it('is disabled for a Member, as the core switches are', async () => {
    h.cores = answer({ canChange: false, boardFilePreview: true })
    await mountWith(4)
    expect(sw(BOARD_FILE_PREVIEW_LABEL).disabled).toBe(true)
    fireEvent.click(sw(BOARD_FILE_PREVIEW_LABEL))
    await settle()
    expect(h.posts).toEqual([])
  })

  it('follows another window: the settings refetch every window gets reads it again', async () => {
    h.cores = answer({ boardFilePreview: false })
    await mountWith(4)
    const before = h.gets.length
    h.cores = answer({ boardFilePreview: true })
    act(() => useSettingsStore.setState({ boardFilePreview: true }))
    await waitFor(() => expect(sw(BOARD_FILE_PREVIEW_LABEL).getAttribute('aria-checked')).toBe('true'))
    expect(h.gets.length).toBe(before + 1)
  })

  it('has no switch on a server from before Board files', async () => {
    h.cores = answer()
    await mountWith(3)
    expect(screen.queryByRole('switch', { name: BOARD_FILE_PREVIEW_LABEL })).toBeNull()
  })

  it('is found by searching for Board', () => {
    const e = MULTIPLAYER_MANIFEST[0]
    expect(e.description).toContain('Board')
    expect(e.keywords).toContain('Board')
  })
})
