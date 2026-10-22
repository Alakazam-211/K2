// @vitest-environment jsdom
//
// prd-garden-access-v1 §8 / G0a / G27–G28: the switch-off golden for
// Settings → Gardens → Local Data
// (`tests/golden/garden_access_off/renderer/local_data_off.html`).
//
// With `gardenAccess` off the page is exactly 0.46.3's: "Local Data", the
// maker lines ("Made by …", "Nobody signed this version"), It's mine, "Let
// <maker> use this data", Grants. No named exception applies to this page.
// The golden was captured from integ/0.46.3-r9 (no Garden access code). On a
// tree with the switch, the switch store is set to off before the page draws.
//
// The fake edges are 0.46.3's ZenWidgetData.test.tsx harness.
//
// Modes: plain run compares and fails loudly on any difference or a missing
// golden. GARDEN_ACCESS_GOLDEN_BLESS=1 writes the golden and prints it as
// base64 between markers (capture only ever from a base WITHOUT Garden access).

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
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

vi.mock('@/lib/garden-data/garden-data-api', () => ({
  listGardenSpaces: vi.fn(async () => ({ spaces: h.spaces.map((s) => ({ ...s })), saves: [] })),
  showGardenSpace: vi.fn(async (widget: string) => {
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
  grantGardenData: vi.fn(async () => {
    throw new Error('the off golden grants nothing')
  }),
  revokeGardenData: vi.fn(async () => {
    throw new Error('the off golden revokes nothing')
  }),
  revokeGardenAdmin: vi.fn(async () => {
    throw new Error('the off golden revokes nothing')
  }),
  claimWidget: vi.fn(async () => {
    throw new Error('the off golden claims nothing')
  }),
  deleteGardenSpace: vi.fn(async () => {
    throw new Error('the off golden deletes nothing')
  }),
  exportGardenData: vi.fn(async () => {
    throw new Error('the off golden exports nothing')
  }),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route === 'zen/widgets') return { ok: true, widgets: h.widgets }
    if (route === 'settings/get') return { gardenAccess: false }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string) => {
    throw new Error(`the off golden posts nothing (POST ${route})`)
  }),
}))

vi.mock('@/lib/zen/zen-api', () => ({
  zenLocalScope: () => ({ id: 'local', kind: 'local' }),
  watchLocalZenChanged: () => () => undefined,
}))

vi.mock('@/stores/session-events', () => ({
  onGardenDataChanged: () => () => undefined,
}))

import { act } from 'react'
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/react'
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { ZenWidgetData } from './ZenWidgetData'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

/** Present only on a tree with Garden access (a glob with no match is {}). */
const SWITCH_MODULE = import.meta.glob('../../../lib/zen/garden-access-api.ts')

const GOLDEN = join(process.cwd(), 'tests/golden/garden_access_off/renderer/local_data_off.html')

function pretty(html: string): string {
  return html.replace(/></g, '>\n<') + '\n'
}

function checkGolden(got: string): void {
  if (process.env.GARDEN_ACCESS_GOLDEN_BLESS === '1') {
    mkdirSync(dirname(GOLDEN), { recursive: true })
    writeFileSync(GOLDEN, got)
    console.log(`GOLDEN-BEGIN local_data_off.html\n${Buffer.from(got).toString('base64')}\nGOLDEN-END`)
    return
  }
  if (!existsSync(GOLDEN)) throw new Error(`no golden at ${GOLDEN}; capture it from a base without Garden access`)
  expect(got).toBe(readFileSync(GOLDEN, 'utf8'))
}

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

beforeEach(async () => {
  // Every kind of line the claim controls touch, plus an orphan.
  h.widgets = [
    { name: 'board', builtin: false, title: 'Board' },
    { name: 'tracker', builtin: false, title: 'Tracker' },
    { name: 'board-lite', builtin: false, madeBy: { kind: 'unclaimed' } },
    { name: 'k2:diary', builtin: true },
  ]
  h.spaces = [
    row('board', { grants: 1, admins: ['board-builder'] }),
    row('tracker', { madeBy: { kind: 'agent', handle: 'board-builder' }, bytes: 45_000, keys: 12 }),
    row('old', { orphanedAt: 5, bytes: 812, keys: 3 }),
  ]
  h.grants = {
    board: [
      {
        id: 7,
        space: 'data:board',
        prefix: '',
        subjectKind: 'workspace',
        subjectId: 'id-ana',
        subjectName: 'ana',
        level: 'read',
        admin: false,
        viaGrant: null,
        grantedBy: 'owner',
        grantedAt: 1,
      },
    ],
  }
  const load = Object.values(SWITCH_MODULE)[0]
  if (load) {
    const mod = (await load()) as { useGardenAccessSwitchStore: { setState(s: { on: boolean; loaded: boolean }): void } }
    mod.useGardenAccessSwitchStore.setState({ on: false, loaded: true })
  }
})

afterEach(() => {
  cleanup()
})

describe('Garden access off: Local Data is 0.46.3', () => {
  it('local_data_off', async () => {
    const { container } = render(<ZenWidgetData />)
    await waitFor(() => expect(container.querySelector('[data-garden-data-line="tracker"]')).not.toBeNull())
    // Open the owner widget's grants too, so the grant list is in the golden.
    const board = container.querySelector('[data-garden-data-line="board"]') as HTMLElement
    fireEvent.click(within(board).getByRole('button', { name: 'Grants' }))
    await waitFor(() => expect(within(board).getByText(/ana/)).toBeTruthy())
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0))
    })
    checkGolden(pretty(container.innerHTML))
  })
})
