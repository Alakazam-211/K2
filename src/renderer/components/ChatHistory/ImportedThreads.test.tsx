// @vitest-environment jsdom
// 0.46.1: Threads that arrived with a clone get an "Imported Threads" group
// in the workspace's Chat History, fed by the daemon's
// `GET /cli/thread/imports?ws=` (rows carry `addr` + `readOnly`). A row
// opens the Thread read-only (no message box). No imports = no group.
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { screen, waitFor, cleanup, fireEvent, act } from '@testing-library/react'

const h = vi.hoisted(() => ({
  imports: { ok: true, imports: [] as unknown[] } as unknown,
  gets: [] as Array<{ route: string; params: unknown }>,
  viewer: [] as Array<{ addr: string; conversationId: string | null; composeBar: unknown }>,
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(
      vi.fn(async (route: string, params?: unknown) => {
        h.gets.push({ route, params })
        if (route === 'thread/imports') return h.imports
        throw new Error(`unexpected GET ${route}`)
      }),
    ),
    daemonCliPost: primaryOnly(vi.fn(async () => ({}))),
  }
})

vi.mock('@/components/SessionView/ThreadOverlayColumn', () => ({
  ThreadOverlayColumn: (p: { addr: string; conversationId: string | null; composeBar: unknown }) => {
    h.viewer.push({ addr: p.addr, conversationId: p.conversationId, composeBar: p.composeBar })
    return <div data-testid="thread-column-stub">{p.addr}</div>
  },
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))

import { ImportedThreadsSection, importedDetail, importedTitle, parseImportedThreads } from './ImportedThreads'

function row(over: Record<string, unknown>): Record<string, unknown> {
  return {
    id: 'r1',
    projectId: 'p1',
    localConversationId: 'conv-main',
    role: 'canonical',
    cloneId: 'c1',
    sourceHost: 'luzz.k2.dev',
    sourceHandle: 'sales',
    sourceConversationId: 'src-main',
    docCount: 412,
    skippedCount: 0,
    importedAt: 100,
    addr: 'sales',
    readOnly: false,
    ...over,
  }
}

beforeEach(() => {
  h.gets.length = 0
  h.viewer.length = 0
  h.imports = { ok: true, imports: [] }
})
afterEach(() => cleanup())

describe('Imported Threads: parse and labels', () => {
  it('keeps the daemon fields, drops rows without an id, newest first', () => {
    const rows = parseImportedThreads({
      imports: [
        row({ id: 'old', importedAt: 1 }),
        row({ id: '', localConversationId: 'x' }),
        row({ id: 'new', importedAt: 9, role: 'detached', localConversationId: 'import:abc', addr: 'import:abc', readOnly: true, name: 'reviewer' }),
      ],
    })
    expect(rows.map((r) => r.id)).toEqual(['new', 'old'])
    expect(rows[0]).toMatchObject({ addr: 'import:abc', readOnly: true, name: 'reviewer', role: 'detached' })
    expect(parseImportedThreads({ error: 'nope' })).toEqual([])
    expect(parseImportedThreads(null)).toEqual([])
  })

  it('titles and details say where each one lives here', () => {
    const [main, side, pending] = parseImportedThreads({
      imports: [
        row({ id: 'a', importedAt: 3 }),
        row({ id: 'b', importedAt: 2, role: 'detached', name: 'reviewer', docCount: 1, addr: 'import:b', readOnly: true }),
        row({ id: 'c', importedAt: 1, role: 'pending_canonical', addr: 'import:c', readOnly: true }),
      ],
    })
    expect(importedTitle(main)).toBe('sales main Thread')
    expect(importedDetail(main)).toBe('in the main Thread · 412 messages · from luzz.k2.dev')
    expect(importedTitle(side)).toBe('reviewer')
    expect(importedDetail(side)).toBe('read-only history · 1 message · from luzz.k2.dev')
    expect(importedDetail(pending)).toBe('joins the main Thread when Chat starts · 412 messages · from luzz.k2.dev')
  })
})

describe('Imported Threads group', () => {
  it('no imports: no group (after asking the daemon for this workspace)', async () => {
    renderInPrimaryRoom(<ImportedThreadsSection projectPath="/work/sales" />)
    await waitFor(() => expect(h.gets).toEqual([{ route: 'thread/imports', params: { ws: '/work/sales' } }]))
    await act(async () => {
      await Promise.resolve()
    })
    expect(screen.queryByTestId('imported-threads')).toBeNull()
    expect(screen.queryByText('Imported Threads')).toBeNull()
  })

  it('lists the imports under "Imported Threads" with the count; a row opens it read-only', async () => {
    h.imports = {
      ok: true,
      imports: [
        row({ id: 'a', importedAt: 5 }),
        row({ id: 'b', importedAt: 4, role: 'detached', name: 'reviewer', localConversationId: 'import:b', addr: 'import:b', readOnly: true }),
      ],
    }
    renderInPrimaryRoom(<ImportedThreadsSection projectPath="/work/sales" />)
    await waitFor(() => expect(screen.getByText('Imported Threads')).toBeTruthy())
    const group = screen.getByTestId('imported-threads')
    expect(group.textContent).toContain('2')
    expect(screen.getByText('sales main Thread')).toBeTruthy()
    fireEvent.click(screen.getByTestId('imported-thread-b'))
    const viewer = await screen.findByTestId('imported-thread-viewer')
    expect(viewer.textContent).toContain('Imported messages are read-only')
    expect(h.viewer.at(-1)).toEqual({ addr: 'import:b', conversationId: 'import:b', composeBar: null })
    fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(screen.queryByTestId('imported-thread-viewer')).toBeNull()
  })

  it('a row with no address here is shown but not openable', async () => {
    h.imports = { ok: true, imports: [row({ id: 'z', role: 'sidecar', addr: null })] }
    renderInPrimaryRoom(<ImportedThreadsSection projectPath="/work/sales" />)
    const btn = (await screen.findByTestId('imported-thread-z')) as HTMLButtonElement
    expect(btn.disabled).toBe(true)
    fireEvent.click(btn)
    expect(screen.queryByTestId('imported-thread-viewer')).toBeNull()
  })
})
