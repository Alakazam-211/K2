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
      return h.cores
    }),
    daemonCliPost: primaryOnly(async (route: string, body: unknown) => {
      h.posts.push({ route, body })
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
import { MultiplayerSection, MULTIPLAYER_INTRO, MULTIPLAYER_MANIFEST, MULTIPLAYER_MEMBER_LINE } from './MultiplayerSection'
import { failedWriteBackLine, openNowLine, parseDocCores, turnOffMessage, turnOffTitle } from './multiplayer-cores-api'
import { searchManifest, SECTION_LABELS } from '../searchManifest'

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
  await waitFor(() => expect(screen.getAllByRole('switch')).toHaveLength(3))
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
      "People and agents work in the same file at the same time, with presence, history and everyone's own undo. Each kind of file has a core. When a core is on, files of that kind open in the multiplayer view on this server, for everyone, with the workspace's agent beside them. When it's off, they open the usual way. The server's owner or an Admin changes these.",
    )
    // The rows are the daemon's, not a hard-coded list.
    expect(screen.getAllByRole('switch').map((s) => s.getAttribute('aria-label'))).toEqual([
      'Markdown core',
      'Text and code core',
      'Extra core',
    ])
    expect(sw('Markdown core').getAttribute('aria-checked')).toBe('false')
    expect(sw('Markdown core').disabled).toBe(false)
    expect(within(row('markdown')).getByText('Beta')).toBeTruthy()
    expect(within(row('markdown')).getByText('Markdown core description.')).toBeTruthy()
    expect(within(row('extra')).queryByText('Beta')).toBeNull()
    expect(screen.queryByText(MULTIPLAYER_MEMBER_LINE)).toBeNull()
    expect(screen.getByTestId('multiplayer-later').textContent).toBe('Coming later: CSV grid, Word, Slides.')
    // Off: no beta line, no open-now line.
    expect(screen.queryByText('Beta: keep your work in git while multiplayer is new.')).toBeNull()
  })

  it('Member: switches visible but disabled, plus the line; a click writes nothing', async () => {
    h.cores = answer({ canChange: false })
    await mount()
    for (const s of screen.getAllByRole('switch')) expect((s as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText("Only the server's owner or an Admin can change these.")).toBeTruthy()
    fireEvent.click(sw('Markdown core'))
    await settle()
    expect(h.posts).toEqual([])
  })

  it('turning on: no dialog, the write body is exact, then a refetch', async () => {
    await mount()
    h.cores = answer({}, { on: true })
    fireEvent.click(sw('Markdown core'))
    await waitFor(() => expect(sw('Markdown core').getAttribute('aria-checked')).toBe('true'))
    expect(screen.queryByText('Turn off the Markdown core?')).toBeNull()
    expect(h.posts).toEqual([{ route: 'settings/update', body: { multiplayerCores: { markdown: true } } }])
    expect(JSON.stringify(h.posts[0].body)).toBe('{"multiplayerCores":{"markdown":true}}')
    expect(h.gets).toEqual(['docs/cores', 'docs/cores'])
    // While on: the beta line under the switch.
    expect(within(row('markdown')).getByText('Beta: keep your work in git while multiplayer is new.')).toBeTruthy()
    expect(within(row('text')).queryByText('Beta: keep your work in git while multiplayer is new.')).toBeNull()
  })

  it('turning off with nothing open flips with no dialog', async () => {
    h.cores = answer({}, { on: true })
    await mount()
    h.cores = answer()
    fireEvent.click(sw('Markdown core'))
    await waitFor(() => expect(h.posts).toHaveLength(1))
    expect(screen.queryByText('Turn off the Markdown core?')).toBeNull()
    expect(h.posts[0]).toEqual({ route: 'settings/update', body: { multiplayerCores: { markdown: false } } })
  })

  it('turning off with files open live asks first: Cancel writes nothing, Turn off writes {markdown:false}', async () => {
    const open = { files: 2, participants: 3, paths: ['notes.md', 'plan.md'] }
    h.cores = answer({}, { on: true, open })
    await mount()
    fireEvent.click(sw('Markdown core'))
    await waitFor(() => expect(screen.getByText('Turn off the Markdown core?')).toBeTruthy())
    expect(
      screen.getByText(
        '2 files are open live (notes.md, plan.md). K2 writes every change to disk, closes them, and they reopen the usual way. Their history stays.',
      ),
    ).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await settle()
    expect(screen.queryByText('Turn off the Markdown core?')).toBeNull()
    expect(h.posts).toEqual([])
    expect(sw('Markdown core').getAttribute('aria-checked')).toBe('true')

    fireEvent.click(sw('Markdown core'))
    await waitFor(() => expect(screen.getByText('Turn off the Markdown core?')).toBeTruthy())
    h.cores = answer()
    fireEvent.click(screen.getByRole('button', { name: 'Turn off' }))
    await waitFor(() => expect(sw('Markdown core').getAttribute('aria-checked')).toBe('false'))
    expect(h.posts).toEqual([{ route: 'settings/update', body: { multiplayerCores: { markdown: false } } }])
  })

  it("shows the daemon's refusal as one line under the row (role=alert)", async () => {
    await mount()
    h.postError = new DaemonHttpError('updating "multiplayerCores" requires the Owner or Admin role', 403, null)
    fireEvent.click(sw('Markdown core'))
    const alert = await within(row('markdown')).findByRole('alert')
    expect(alert.textContent).toBe('updating "multiplayerCores" requires the Owner or Admin role')
    expect(within(row('text')).queryByRole('alert')).toBeNull()
    expect(sw('Markdown core').getAttribute('aria-checked')).toBe('false')
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
    await waitFor(() => expect(sw('Markdown core').getAttribute('aria-checked')).toBe('true'))
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
      "notes.md couldn't be written when the Markdown core was turned off (no space left on the disk). Its last changes are kept in K2's history; turning the Markdown core on again writes them.",
      "docs/plan.md couldn't be written when the Markdown core was turned off (permission denied). Its last changes are kept in K2's history; turning the Markdown core on again writes them.",
    ])
    cleanup()
    h.cores = answer({}, { on: true, lastChange: { ...lastChange, on: false } })
    await mount()
    expect(screen.queryByTestId('multiplayer-failed-markdown')).toBeNull()
  })

  it('a malformed answer is an error, never a guess', async () => {
    expect(() => parseDocCores({ ok: true, canChange: true, cores: [{ id: 'markdown' }] })).toThrow(
      'docs/cores: cores[0].name is not a string',
    )
    expect(() => parseDocCores({ ok: true, cores: [] })).toThrow('docs/cores: the answer.canChange is not true or false')
    h.cores = { ok: true, canChange: 'yes', cores: [] }
    render(<MultiplayerSection />)
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toBe('docs/cores: the answer.canChange is not true or false')
  })

  it('search finds "Multiplayer" by name and keywords', () => {
    expect(SECTION_LABELS.multiplayer).toBe('Multiplayer')
    expect(MULTIPLAYER_MANIFEST.map((e) => [e.id, e.section, e.label])).toEqual([['multiplayer.cores', 'multiplayer', 'Multiplayer']])
    expect(MULTIPLAYER_MANIFEST[0].keywords).toEqual(['live docs', 'live editing', 'multiplayer', 'Markdown', 'cores', 'co-editing'])
    for (const q of ['Multiplayer', 'live docs', 'live editing', 'markdown', 'co-editing']) {
      expect(searchManifest(MULTIPLAYER_MANIFEST, q).map((e) => e.id)).toEqual(['multiplayer.cores'])
    }
  })
})

describe('core names in a sentence', () => {
  it('reads "the Markdown core", and quotes a name that is not a core', () => {
    expect(turnOffTitle({ name: 'Markdown core' })).toBe('Turn off the Markdown core?')
    expect(turnOffTitle({ name: 'Any other file' })).toBe('Turn off the "Any other file" core?')
    expect(failedWriteBackLine({ name: 'Text and code core' }, { path: 'a.txt', error: 'no space left on the disk' })).toBe(
      "a.txt couldn't be written when the Text and code core was turned off (no space left on the disk). " +
        "Its last changes are kept in K2's history; turning the Text and code core on again writes them.",
    )
  })
})
