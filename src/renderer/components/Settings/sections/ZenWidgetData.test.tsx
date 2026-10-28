// @vitest-environment jsdom
//
// Settings → Gardens → Local Data (contract-garden-data-v1 §11.4, §12.3).
// The real component; only the edges are faked: the Garden data API (an
// in-memory model of `spaces` / `show` / grant changes / export pages),
// `GET zen/widgets`, and the local `garden_data_changed` / `zen_changed`
// subscriptions.
//
// Asserted (fail loudly): size, key count and grants per line; the maker
// ("Made by …", "Nobody signed this version"); It's mine for an agent or an
// unclaimed version (never for the person's own); "Let <maker> use this
// data" only while the agent maker has no grant, and it grants write on
// everything; the grant list with "by <grant admin>" and the project
// convenience note; removing a grant admin dry-runs first and names the
// cascade; Export writes the JSONL header + rows across pages; Delete asks
// in K2's confirm dialog and sends `confirm` = the widget; orphans list
// under "Data with no widget"; a `garden_data_changed` re-reads.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type Row = {
  widget: string
  space: string
  bytes: number
  keys: number
  rev: number
  updatedAt: number
  orphanedAt: number | null
  grants: number
  admins: string[]
  madeBy: { kind: 'owner' } | { kind: 'k2' } | { kind: 'agent'; handle: string } | { kind: 'unclaimed' } | null
}

const h = vi.hoisted(() => ({
  spaces: [] as Row[],
  widgets: [] as Array<Record<string, unknown>>,
  grants: {} as Record<string, Array<Record<string, unknown>>>,
  calls: [] as Array<[string, unknown]>,
  dataHandlers: [] as Array<(space: string, rev: number) => void>,
  zenHandlers: [] as Array<() => void>,
  exportPages: {} as Record<string, Array<{ items: unknown[]; next: string | null }>>,
  downloads: [] as Array<{ name: string; text: string }>,
  blobs: {} as Record<string, Array<{ ref: string; size: number; mime: string; name: string | null; refs: number; pending: boolean; bytes: number[] }>>,
  history: {} as Record<string, Array<Record<string, unknown>>>,
  restoreFamilies: {} as Record<string, string[]>,
}))

vi.mock('@/lib/garden-data/garden-data-api', () => ({
  listGardenSpaces: vi.fn(async () => {
    h.calls.push(['spaces', null])
    return { spaces: h.spaces.map((s) => ({ ...s })), saves: [] }
  }),
  showGardenSpace: vi.fn(async (widget: string) => {
    h.calls.push(['show', widget])
    const s = h.spaces.find((x) => x.widget === widget)
    if (!s) throw new Error('space_not_found')
    return {
      space: { space: s.space, widget, rev: s.rev, reapedRev: 0, bytes: s.bytes, keys: s.keys, createdAt: 1, updatedAt: 1, orphanedAt: s.orphanedAt },
      grants: h.grants[widget] ?? [],
      delegations: [],
      madeBy: s.madeBy,
      liveHash: 'a'.repeat(64),
    }
  }),
  grantGardenData: vi.fn(async (body: Record<string, unknown>) => {
    h.calls.push(['grant', body])
    const list = (h.grants[String(body.widget)] ??= [])
    const g = {
      id: 100 + list.length,
      space: `data:${String(body.widget)}`,
      prefix: body.prefix ?? '',
      subjectKind: 'workspace',
      subjectId: `id-${String(body.agent)}`,
      subjectName: body.agent,
      level: body.level,
      admin: false,
      viaGrant: null,
      grantedBy: 'owner',
      grantedAt: 1,
    }
    list.push(g)
    const s = h.spaces.find((x) => x.widget === body.widget)
    if (s) s.grants += 1
    return { grant: g, created: true }
  }),
  revokeGardenData: vi.fn(async (body: Record<string, unknown>) => {
    h.calls.push(['revoke', body])
    return { revoked: 1 }
  }),
  revokeGardenAdmin: vi.fn(async (body: Record<string, unknown>) => {
    h.calls.push(['unadmin', body])
    return { revoked: body.dryRun ? 0 : 1, cascaded: 2 }
  }),
  claimWidget: vi.fn(async (widget: string) => {
    h.calls.push(['claim', widget])
    const s = h.spaces.find((x) => x.widget === widget)
    if (s) s.madeBy = { kind: 'owner' }
    return { widget, hash: 'a'.repeat(64) }
  }),
  deleteGardenSpace: vi.fn(async (widget: string, confirm: string) => {
    h.calls.push(['delete', { widget, confirm }])
    h.spaces = h.spaces.filter((x) => x.widget !== widget)
    return { deleted: `data:${widget}`, snapshot: 'gardens-x-delete.sqlite', grantsRemoved: 0 }
  }),
  // 0.46.6 (prd-garden-board-needs-v1 GB17, GH6): files, history, restore.
  listGardenBlobs: vi.fn(async (widget: string) => {
    h.calls.push(['blobs', widget])
    const list = h.blobs[widget] ?? []
    return { items: list.map(({ bytes: _b, ...i }) => i), next: null, bytes: list.reduce((n, b) => n + b.size, 0), quota: 256 * 1024 * 1024 }
  }),
  readGardenBlob: vi.fn(async (widget: string, ref: string, offset: number) => {
    const b = (h.blobs[widget] ?? []).find((x) => x.ref === ref)
    if (!b) throw new Error('blob_missing')
    const part = b.bytes.slice(offset)
    return { base64: btoa(String.fromCharCode(...part)), len: part.length, eof: true, size: b.size }
  }),
  gardenHistory: vi.fn(async (widget: string) => {
    h.calls.push(['history', widget])
    return { head: 9, oldest: 2, items: h.history[widget] ?? [], next: null }
  }),
  restoreGardenData: vi.fn(async (body: Record<string, unknown>) => {
    h.calls.push(['restore', body])
    const all = h.restoreFamilies[String(body.widget)] ?? []
    const prefixes = (body.prefixes as string[] | undefined) ?? []
    const families = prefixes.length ? all.filter((f) => prefixes.includes(f)) : all
    const dry = body.dryRun === true
    return { id: dry ? null : 'rs-0123456789ab', rev: dry ? null : 10, written: families.length, deleted: 0, families, keys: [] }
  }),
  exportGardenData: vi.fn(async (widget: string, opts: { after?: string } = {}) => {
    h.calls.push(['export', { widget, ...opts }])
    const pages = h.exportPages[widget] ?? [{ items: [], next: null }]
    const i = opts.after ? pages.findIndex((_, n) => n > 0 && pages[n - 1].next === opts.after) : 0
    if (i < 0) throw new Error(`no page after ${opts.after}`)
    return { header: { space: `data:${widget}`, rev: 9, exportedAt: 100, snapshot: null }, items: pages[i].items, next: pages[i].next }
  }),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route !== 'zen/widgets') throw new Error(`unexpected GET ${route}`)
    h.calls.push(['widgets', null])
    return { ok: true, widgets: h.widgets }
  }),
}))

vi.mock('@/lib/zen/zen-api', () => ({
  zenLocalScope: () => ({ id: 'local', kind: 'local' }),
  watchLocalZenChanged: (fn: () => void) => {
    h.zenHandlers.push(fn)
    return () => undefined
  },
}))

vi.mock('@/stores/session-events', () => ({
  onGardenDataChanged: (_scope: unknown, fn: (space: string, rev: number) => void) => {
    h.dataHandlers.push(fn)
    return () => undefined
  },
}))

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import ConfirmDialog from '@/components/ConfirmDialog/ConfirmDialog'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { buildDataLines, exportJsonl, formatBytes, makerText, ZenWidgetData } from './ZenWidgetData'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function row(widget: string, extra: Partial<Row> = {}): Row {
  return {
    widget,
    space: `data:${widget}`,
    bytes: 2_100_000,
    keys: 812,
    rev: 41,
    updatedAt: 1,
    orphanedAt: null,
    grants: 0,
    admins: [],
    madeBy: { kind: 'owner' },
    ...extra,
  }
}

function line(widget: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-garden-data-line="${widget}"]`)
  if (!el) throw new Error(`no data line for ${widget}\n${document.body.innerHTML.slice(0, 2000)}`)
  return el
}

async function mount(): Promise<void> {
  render(
    <>
      <ZenWidgetData />
      <ConfirmDialog />
    </>,
  )
  await waitFor(() => expect(document.querySelector('[data-garden-data]')?.textContent).not.toContain('Loading…'))
}

beforeEach(() => {
  h.spaces = []
  h.widgets = []
  h.grants = {}
  h.calls = []
  h.dataHandlers = []
  h.zenHandlers = []
  h.exportPages = {}
  h.downloads = []
  h.blobs = {}
  h.history = {}
  h.restoreFamilies = {}
  Object.assign(URL, { createObjectURL: () => 'blob:x', revokeObjectURL: () => undefined })
  anchorClick = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
    h.downloads.push({ name: this.download, text: '' })
  })
})

let anchorClick: { mockRestore(): void } | null = null

afterEach(() => {
  cleanup()
  anchorClick?.mockRestore()
  useConfirmDialogStore.setState({ isOpen: false, onResolve: null })
})

describe('pure helpers', () => {
  it('formats sizes and makers', () => {
    expect(formatBytes(812)).toBe('812 B')
    expect(formatBytes(2_100_000)).toBe('2.1 MB')
    expect(formatBytes(45_000)).toBe('45 KB')
    expect(makerText({ kind: 'agent', handle: 'board-builder' })).toBe('Made by board-builder')
    expect(makerText({ kind: 'unclaimed' })).toBe('Nobody signed this version')
    expect(makerText({ kind: 'owner' })).toBe('Made by you')
    expect(makerText(null)).toBeNull()
  })

  it('lines: custom widgets with or without data; built-ins and orphans are not lines', () => {
    const lines = buildDataLines(
      [
        { name: 'board', builtin: false, title: 'Board' },
        { name: 'fresh', builtin: false, madeBy: { kind: 'unclaimed' } },
        { name: 'k2:diary', builtin: true },
      ],
      { spaces: [row('board'), row('old', { orphanedAt: 5 }), row('quiet')], saves: [] },
    )
    expect(lines.map((l) => [l.widget, l.space?.keys ?? null, l.madeBy?.kind ?? null])).toEqual([
      ['board', 812, 'owner'],
      ['fresh', null, 'unclaimed'],
      ['quiet', 812, 'owner'],
    ])
  })

  // 0.46.5 WS14: Settings shows the same "Large widget" note next to the name.
  it('lines: a widget above 512 KB of code carries the Large widget note; at 512 KB none', () => {
    const size = (code: number) => ({ code, codeWarnAt: 512 * 1024, codeMax: 1024 * 1024 })
    const lines = buildDataLines(
      [
        { name: 'big', builtin: false, size: size(740 * 1024) },
        { name: 'edge', builtin: false, size: size(512 * 1024) },
        { name: 'old', builtin: false },
      ],
      { spaces: [], saves: [] },
    )
    expect(lines.map((l) => [l.widget, l.large])).toEqual([
      ['big', 'Large widget: 740 KB of code (limit 1 MB). Ask your agent about splitting it or making it an App.'],
      ['edge', null],
      ['old', null],
    ])
  })
})

describe('ZenWidgetData', () => {
  it('0.46.5 WS14: a large widget row says so next to its name', async () => {
    h.widgets = [{ name: 'big', builtin: false, size: { code: 740 * 1024, codeWarnAt: 512 * 1024, codeMax: 1024 * 1024 } }]
    await mount()
    const note = line('big').querySelector('[data-zen-large-widget="big"]')
    expect(note?.textContent).toBe('Large widget: 740 KB of code (limit 1 MB). Ask your agent about splitting it or making it an App.')
  })

  it('shows size, keys, grants, grant admin and the maker', async () => {
    h.widgets = [{ name: 'board', builtin: false, title: 'Board' }]
    h.spaces = [row('board', { grants: 3, admins: ['board-builder'] })]
    await mount()
    const summary = within(line('board')).getByText(/2.1 MB/)
    expect(summary.textContent).toBe('2.1 MB · 812 keys · 3 grants · grant admin: board-builder · Made by you')
    expect(within(line('board')).queryByText("It's mine")).toBeNull()
    expect(within(line('board')).queryByText(/use this data/)).toBeNull()
  })

  it("an unclaimed widget with no data yet offers It's mine", async () => {
    h.widgets = [{ name: 'board-lite', builtin: false, madeBy: { kind: 'unclaimed' } }]
    await mount()
    expect(within(line('board-lite')).getByText(/No data yet/).textContent).toContain('Nobody signed this version')
    expect(within(line('board-lite')).queryByText('Export')).toBeNull()
    await act(async () => {
      fireEvent.click(within(line('board-lite')).getByText("It's mine"))
    })
    expect(h.calls).toContainEqual(['claim', 'board-lite'])
  })

  it('an agent maker with no grant: Let <maker> use this data grants write on everything, once', async () => {
    h.widgets = [{ name: 'board-lite', builtin: false }]
    h.spaces = [row('board-lite', { madeBy: { kind: 'agent', handle: 'board-builder' } })]
    await mount()
    const btn = within(line('board-lite')).getByText('Let board-builder use this data')
    expect(within(line('board-lite')).getByText("It's mine")).toBeTruthy()
    await act(async () => {
      fireEvent.click(btn)
    })
    expect(h.calls).toContainEqual(['grant', { widget: 'board-lite', agent: 'board-builder', level: 'write', prefix: '' }])
    await waitFor(() => expect(within(line('board-lite')).queryByText('Let board-builder use this data')).toBeNull())
  })

  it('an agent maker that already has a grant is not offered one', async () => {
    h.widgets = [{ name: 'board', builtin: false }]
    h.spaces = [row('board', { grants: 1, madeBy: { kind: 'agent', handle: 'board-builder' } })]
    h.grants.board = [
      { id: 1, space: 'data:board', prefix: 'project/7/', subjectKind: 'workspace', subjectId: 'w-1', subjectName: 'board-builder', level: 'write', admin: false, viaGrant: null, grantedBy: 'owner', grantedAt: 1 },
    ]
    await mount()
    await waitFor(() => expect(h.calls.some((c) => c[0] === 'show')).toBe(true))
    await waitFor(() => expect(within(line('board')).queryByText(/use this data/)).toBeNull())
  })

  it('grants list: by the grant admin, project convenience, revoke and remove-admin with its cascade', async () => {
    h.widgets = [{ name: 'board', builtin: false }]
    h.spaces = [row('board', { grants: 3, admins: ['board-builder'] })]
    h.grants.board = [
      { id: 7, space: 'data:board', prefix: '', subjectKind: 'workspace', subjectId: 'w-b', subjectName: 'board-builder', level: 'write', admin: true, viaGrant: null, grantedBy: 'owner', grantedAt: 1 },
      { id: 8, space: 'data:board', prefix: 'project/7/', subjectKind: 'workspace', subjectId: 'w-k', subjectName: 'board-keeper', level: 'write', admin: false, viaGrant: 7, grantedBy: 'agent:w-b', grantedAt: 2 },
      { id: 9, space: 'data:board', prefix: '', subjectKind: 'project', subjectId: 'p-q4', subjectName: 'q4', level: 'read', admin: false, viaGrant: null, grantedBy: 'owner', grantedAt: 3 },
    ]
    await mount()
    await act(async () => {
      fireEvent.click(within(line('board')).getByText('Grants'))
    })
    const keeper = await waitFor(() => {
      const el = document.querySelector<HTMLElement>('[data-garden-data-grant="board-keeper"]')
      if (!el) throw new Error('no keeper grant')
      return el
    })
    expect(keeper.textContent).toContain('board-keeper · write · project/7/')
    expect(keeper.textContent).toContain('by board-builder')
    const project = document.querySelector<HTMLElement>('[data-garden-data-grant="project q4"]')
    expect(project?.textContent).toContain('by you')
    expect(project?.textContent).toContain('convenience: any agent can join a project')
    await act(async () => {
      fireEvent.click(within(keeper).getByText('Revoke'))
    })
    expect(h.calls).toContainEqual(['revoke', { widget: 'board', agent: 'w-k', prefix: 'project/7/' }])
    await act(async () => {
      fireEvent.click(within(project as HTMLElement).getByText('Revoke'))
    })
    expect(h.calls).toContainEqual(['revoke', { widget: 'board', project: 'p-q4', prefix: '' }])
    // Remove the grant admin: dry run, the confirm names the cascade, then the real call.
    const admin = document.querySelector<HTMLElement>('[data-garden-data-admin="board-builder"]') as HTMLElement
    await act(async () => {
      fireEvent.click(within(admin).getByText('Remove'))
    })
    expect(h.calls).toContainEqual(['unadmin', { widget: 'board', agent: 'w-b', prefix: '', dryRun: true }])
    await waitFor(() => expect(screen.getByText('2 grants board-builder made go with it.')).toBeTruthy())
    await act(async () => {
      // The dialog's button (the row's own Remove comes first).
      const removes = screen.getAllByRole('button', { name: 'Remove' })
      expect(removes).toHaveLength(2)
      fireEvent.click(removes[1])
    })
    expect(h.calls).toContainEqual(['unadmin', { widget: 'board', agent: 'w-b', prefix: '' }])
  })

  it('Export writes the JSONL header then every page', async () => {
    h.exportPages.board = [
      { items: [{ key: 'obj/1', rev: 2, value: { t: 'a' }, updatedAt: 2, by: 'you' }], next: 'obj/1' },
      { items: [{ key: 'obj/2', rev: 3, value: 1, updatedAt: 3, by: 'agent:board-keeper' }], next: null },
    ]
    const text = await exportJsonl('board')
    expect(text.trim().split('\n').map((l) => JSON.parse(l))).toEqual([
      { space: 'data:board', rev: 9, exportedAt: 100, snapshot: null },
      { key: 'obj/1', rev: 2, value: { t: 'a' }, updatedAt: 2, by: 'you' },
      { key: 'obj/2', rev: 3, value: 1, updatedAt: 3, by: 'agent:board-keeper' },
    ])
    h.widgets = [{ name: 'board', builtin: false }]
    h.spaces = [row('board')]
    await mount()
    await act(async () => {
      fireEvent.click(within(line('board')).getByText('Export'))
    })
    await waitFor(() => expect(h.downloads).toEqual([{ name: 'board-data.jsonl', text: '' }]))
  })

  it('Delete asks first; No keeps the data, Delete data sends confirm = the widget', async () => {
    h.widgets = [{ name: 'board', builtin: false }]
    h.spaces = [row('board')]
    await mount()
    await act(async () => {
      fireEvent.click(within(line('board')).getByText('Delete'))
    })
    await waitFor(() => expect(screen.getByText("Delete board's data?")).toBeTruthy())
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    })
    expect(h.calls.some((c) => c[0] === 'delete')).toBe(false)
    await act(async () => {
      fireEvent.click(within(line('board')).getByText('Delete'))
    })
    await act(async () => {
      fireEvent.click(await screen.findByRole('button', { name: 'Delete data' }))
    })
    expect(h.calls).toContainEqual(['delete', { widget: 'board', confirm: 'board' }])
    await waitFor(() => expect(within(line('board')).getByText(/No data yet/)).toBeTruthy())
  })

  it('orphans list under Data with no widget; a garden_data_changed re-reads', async () => {
    h.widgets = []
    h.spaces = [row('board-old', { orphanedAt: 5, bytes: 2_100_000 })]
    await mount()
    const orphan = document.querySelector<HTMLElement>('[data-garden-data-orphan="board-old"]')
    expect(orphan?.textContent).toContain('board-old (2.1 MB)')
    expect(screen.getByText('Data with no widget')).toBeTruthy()
    expect(document.querySelector('[data-garden-data-line]')).toBeNull()
    const before = h.calls.filter((c) => c[0] === 'spaces').length
    h.spaces = [row('board-old')]
    h.widgets = [{ name: 'board-old', builtin: false }]
    await act(async () => {
      for (const fn of h.dataHandlers) fn('data:board-old', 42)
      await new Promise((r) => setTimeout(r, 300))
    })
    expect(h.calls.filter((c) => c[0] === 'spaces').length).toBe(before + 1)
    expect(document.querySelector('[data-garden-data-orphans]')).toBeNull()
    expect(line('board-old')).toBeTruthy()
  })

  it('a load failure shows inline with Try again', async () => {
    const api = await import('@/lib/garden-data/garden-data-api')
    vi.mocked(api.listGardenSpaces).mockRejectedValueOnce(new Error('store_unavailable'))
    render(<ZenWidgetData />)
    const err = await screen.findByTestId('garden-data-load-error')
    expect(err.textContent).toBe('store_unavailable')
    await act(async () => {
      fireEvent.click(screen.getByText('Try again'))
    })
    await waitFor(() => expect(screen.queryByTestId('garden-data-load-error')).toBeNull())
  })
})

describe('0.46.6: files, export with images, restore (prd-garden-board-needs-v1 GB17, GH6, R2b)', () => {
  it('shows the files line, and zips data.jsonl with every file a key names', async () => {
    h.spaces = [row('board')]
    h.widgets = [{ name: 'board', builtin: false }]
    const held = 'ab'.repeat(32)
    h.blobs.board = [
      { ref: `k2blob:sha256-${held}`, size: 3, mime: 'image/png', name: 'a.png', refs: 1, pending: false, bytes: [1, 2, 3] },
      { ref: `k2blob:sha256-${'cd'.repeat(32)}`, size: 2, mime: 'image/png', name: 'gone.png', refs: 0, pending: true, bytes: [9, 9] },
    ]
    const zips: Uint8Array[] = []
    const RealBlob = globalThis.Blob
    class SpyBlob extends RealBlob {
      constructor(parts?: BlobPart[], opts?: BlobPropertyBag) {
        super(parts, opts)
        if (opts?.type === 'application/zip' && parts) zips.push(parts[0] as Uint8Array)
      }
    }
    globalThis.Blob = SpyBlob as typeof Blob
    try {
      await mount()
      await waitFor(() => expect(line('board').querySelector('[data-garden-data-files]')?.textContent).toBe('Files: 1 KiB of 256.0 MiB'))
      await act(async () => {
        fireEvent.click(within(line('board')).getByText('Export with images'))
      })
      await waitFor(() => expect(h.downloads.map((d) => d.name)).toEqual(['board-data.zip']))
    } finally {
      globalThis.Blob = RealBlob
    }
    expect(zips).toHaveLength(1)
    const text = new TextDecoder().decode(zips[0])
    expect(text).toContain('data.jsonl')
    expect(text).toContain(`blobs/${held}`)
    expect(text).not.toContain('cd'.repeat(32))
  })

  it('Restore… lists the changes, offers each family, warns on a wide restore, and restores only what is chosen', async () => {
    h.spaces = [row('board')]
    h.widgets = [{ name: 'board', builtin: false }]
    h.history.board = [
      { key: 'board/b1/obj/1', rev: 9, deleted: false, value: 2, prev: 1, prevKind: 'value', by: 'you via board', at: 1_760_000_000 },
      { key: 'settings/zoom', rev: 2, deleted: false, value: 1, prev: null, prevKind: 'none', by: 'you', at: 1_759_000_000 },
    ]
    h.restoreFamilies.board = ['board/', 'settings/']
    await mount()
    await act(async () => {
      fireEvent.click(within(line('board')).getByText('Restore…'))
    })
    const panel = await waitFor(() => {
      const p = line('board').querySelector<HTMLElement>('[data-garden-data-restore]')
      if (!p) throw new Error('no restore panel')
      return p
    })
    await waitFor(() => expect(panel.querySelectorAll('input[type=radio]')).toHaveLength(2))
    const radios = panel.querySelectorAll<HTMLInputElement>('input[type=radio]')
    expect(radios[1].disabled).toBe(true)
    await act(async () => {
      fireEvent.click(radios[0])
    })
    await waitFor(() =>
      expect(panel.querySelector('[data-garden-data-restore-warning]')?.textContent).toContain('access you removed may come back'),
    )
    const select = panel.querySelector<HTMLSelectElement>('[data-garden-data-family]')
    if (!select) throw new Error('no family picker')
    await waitFor(() => expect([...select.options].map((o) => o.textContent)).toEqual(['Everything', 'board/', 'settings/']))
    await act(async () => {
      fireEvent.change(select, { target: { value: 'board/' } })
    })
    await waitFor(() => expect(panel.querySelector('[data-garden-data-restore-warning]')).toBeNull())
    await act(async () => {
      fireEvent.click(within(panel).getByRole('button', { name: 'Restore' }))
    })
    await waitFor(() =>
      expect(h.calls.filter(([k, b]) => k === 'restore' && !(b as { dryRun?: boolean }).dryRun)).toEqual([
        ['restore', { widget: 'board', toRev: 8, prefixes: ['board/'] }],
      ]),
    )
  })
})
