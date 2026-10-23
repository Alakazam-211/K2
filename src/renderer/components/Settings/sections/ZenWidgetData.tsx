// Settings → Gardens → Local Data (prd-gardens-local-services-v1 §4.6,
// §5.8; contract-garden-data-v1 §11.4, B3). The body of its own page in the
// Gardens group from 0.45.7 (`ZenWidgetDataSection` gates it on Gardens
// being set up); before, a block under the Gardens list. One Data line per custom
// widget on this computer:
//   - size and key count (or "No data yet");
//   - who made the live version ("Made by you" / "Made by board-builder" /
//     "Nobody signed this version") and, for an agent or nobody, It's mine;
//   - "Let <maker> use this data" when an agent maker has no grant;
//   - the grants (agent and project; a project grant is marked as a
//     convenience), the grant admin, and which grants it made ("by …");
//   - Export (JSONL, the same file `k2 zen data export` writes) and Delete
//     (K2's confirm dialog; a snapshot is kept first).
// Below: "Data with no widget" (a space whose folder is gone): Export ·
// Delete. Every call goes to THIS computer's daemon (`garden-data-api.ts`,
// owner token, local scope). Re-reads on a local `garden_data_changed` and
// `zen_changed`.
//
// prd-garden-access-v1 G28 (Garden access ON only; off = exactly 0.46.3):
// the page is "Widget data"; It's mine and "Let <maker> use this data" are
// gone (a Garden's list is how an agent gets a widget's data now); the line
// names who last signed the version, never "Nobody signed" (that lives in
// the widget's history modal); the grants fold under Advanced.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import {
  claimWidget,
  deleteGardenSpace,
  exportGardenData,
  grantGardenData,
  listGardenSpaces,
  revokeGardenAdmin,
  revokeGardenData,
  showGardenSpace,
} from '@/lib/garden-data/garden-data-api'
import type {
  GardenGrantRow,
  GardenMadeBy,
  GardenShowResponse,
  GardenSpaceRow,
  GardenSpacesResponse,
} from '@/lib/garden-data/garden-data-types'
import { watchLocalZenChanged, zenLocalScope } from '@/lib/zen/zen-api'
import { onGardenDataChanged } from '@/stores/session-events'
import { useGardenAccessSwitchStore } from '@/lib/zen/garden-access-api'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'

const BTN =
  'px-2 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default'
const DANGER_BTN =
  'px-2 py-1 text-xs border border-[var(--color-status-error)] text-[var(--color-status-error)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default'
const MUTED = 'text-[11px] text-[var(--color-text-muted)]'

/** One `GET zen/widgets` row, as far as this page reads it. `madeBy` is
 *  read when the daemon sends it (a widget with no data yet has no `spaces`
 *  row to carry its maker). */
interface WidgetRow {
  name: string
  builtin: boolean
  title?: string | null
  madeBy?: GardenMadeBy | null
}

/** A Data line: a custom widget, with its data row when it has one. */
export interface DataLineModel {
  widget: string
  title: string | null
  space: GardenSpaceRow | null
  madeBy: GardenMadeBy | null
}

/** "2.1 MB", "812 B": the same units Finder uses (1000-based). */
export function formatBytes(n: number): string {
  if (n < 1000) return `${n} B`
  const units = ['KB', 'MB', 'GB']
  let v = n / 1000
  let i = 0
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000
    i += 1
  }
  return `${v >= 10 ? Math.round(v) : Math.round(v * 10) / 10} ${units[i]}`
}

/** Who made the live version, in words. */
export function makerText(m: GardenMadeBy | null): string | null {
  if (!m) return null
  switch (m.kind) {
    case 'owner':
      return 'Made by you'
    case 'k2':
      return 'Built into K2'
    case 'agent':
      return `Made by ${m.handle}`
    case 'unclaimed':
      return 'Nobody signed this version'
  }
}

/** Who last signed the live version (G28, Garden access on), or null. */
export function signerText(m: GardenMadeBy | null): string | null {
  if (!m) return null
  switch (m.kind) {
    case 'owner':
      return 'Last signed by you'
    case 'k2':
      return 'Built into K2'
    case 'agent':
      return `Last signed by ${m.handle}`
    case 'unclaimed':
      return null
  }
}

/** The Data lines: every custom widget (with or without data), then any
 *  data row whose widget is listed only by `spaces` and still has a folder.
 *  Orphans (`orphanedAt`) are not lines; they're "Data with no widget". */
export function buildDataLines(widgets: readonly WidgetRow[], spaces: GardenSpacesResponse): DataLineModel[] {
  const byWidget = new Map(spaces.spaces.map((s) => [s.widget, s]))
  const out: DataLineModel[] = []
  const seen = new Set<string>()
  for (const w of widgets) {
    if (w.builtin) continue
    const s = byWidget.get(w.name) ?? null
    if (s?.orphanedAt != null) continue
    seen.add(w.name)
    out.push({ widget: w.name, title: w.title ?? null, space: s, madeBy: s?.madeBy ?? w.madeBy ?? null })
  }
  for (const s of spaces.spaces) {
    if (seen.has(s.widget) || s.orphanedAt != null) continue
    out.push({ widget: s.widget, title: null, space: s, madeBy: s.madeBy })
  }
  out.sort((a, b) => a.widget.localeCompare(b.widget))
  return out
}

/** The subject of a grant, in words. */
function subjectText(g: GardenGrantRow): string {
  const name = g.subjectName ?? g.subjectId
  return g.subjectKind === 'project' ? `project ${name}` : name
}

/** "you" or the grant admin's name, for "by …". */
function madeByText(g: GardenGrantRow, rows: readonly GardenGrantRow[]): string {
  if (g.viaGrant === null) return 'you'
  const d = rows.find((r) => r.id === g.viaGrant)
  return d ? (d.subjectName ?? d.subjectId) : 'a grant admin'
}

/** True when the agent `handle` has its own grant (or delegation) here. */
export function makerHasGrant(handle: string, detail: GardenShowResponse | null, space: GardenSpaceRow | null): boolean {
  if (!space || space.grants === 0) return space?.admins.includes(handle) ?? false
  if (space.admins.includes(handle)) return true
  if (!detail) return true // not known yet: don't offer a grant that may exist
  return detail.grants.some((g) => g.subjectKind === 'workspace' && (g.subjectName === handle || g.subjectId === handle))
}

function errText(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

/** Every export page as one JSONL text (header line first). */
export async function exportJsonl(widget: string): Promise<string> {
  let after: string | undefined
  let text = ''
  for (let first = true; ; first = false) {
    const page = await exportGardenData(widget, after ? { after } : {})
    if (first) text += JSON.stringify(page.header) + '\n'
    for (const it of page.items) {
      text += JSON.stringify({ key: it.key, rev: it.rev, value: it.value, updatedAt: it.updatedAt, by: it.by }) + '\n'
    }
    if (!page.next) return text
    after = page.next
  }
}

function download(name: string, text: string): void {
  const blob = new Blob([text], { type: 'application/x-ndjson' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = name
  a.click()
  URL.revokeObjectURL(url)
}

function useAction(): {
  busy: boolean
  error: string | null
  run: (fn: () => Promise<unknown>, done?: string) => Promise<boolean>
} {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const run = useCallback(async (fn: () => Promise<unknown>, done?: string): Promise<boolean> => {
    setBusy(true)
    setError(null)
    try {
      await fn()
      if (done) useToastStore.getState().addToast(done, 'success')
      return true
    } catch (err) {
      setError(errText(err))
      return false
    } finally {
      setBusy(false)
    }
  }, [])
  return { busy, error, run }
}

async function exportAndSave(widget: string): Promise<void> {
  const text = await exportJsonl(widget)
  download(`${widget.replace(/[^A-Za-z0-9._-]/g, '_')}-data.jsonl`, text)
}

async function confirmDelete(widget: string, bytes: number): Promise<boolean> {
  return useConfirmDialogStore.getState().confirm({
    title: `Delete ${widget}'s data?`,
    message: `This removes every key (${formatBytes(bytes)}) and every grant for ${widget}. K2 keeps a snapshot first. The widget itself stays.`,
    confirmLabel: 'Delete data',
    destructive: true,
  })
}

function GrantList({
  widget,
  detail,
  reload,
}: {
  widget: string
  detail: GardenShowResponse
  reload: () => void
}): React.JSX.Element {
  const act = useAction()
  const rows = detail.grants
  const plain = rows.filter((g) => !g.admin)
  const admins = rows.filter((g) => g.admin)
  const revoke = (g: GardenGrantRow): void =>
    void act
      .run(() =>
        revokeGardenData({
          widget,
          ...(g.subjectKind === 'project' ? { project: g.subjectId } : { agent: g.subjectId }),
          prefix: g.prefix,
        }),
      )
      .then((ok) => ok && reload())
  const unadmin = async (g: GardenGrantRow): Promise<void> => {
    const body = { widget, agent: g.subjectId, prefix: g.prefix }
    let cascaded = 0
    const counted = await act.run(async () => {
      cascaded = (await revokeGardenAdmin({ ...body, dryRun: true })).cascaded
    })
    if (!counted) return
    const name = g.subjectName ?? g.subjectId
    const yes = await useConfirmDialogStore.getState().confirm({
      title: `Stop ${name} being a grant admin?`,
      message:
        cascaded === 0
          ? `${name} made no grants here.`
          : `${cascaded} grant${cascaded === 1 ? '' : 's'} ${name} made go${cascaded === 1 ? 'es' : ''} with it.`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (yes && (await act.run(() => revokeGardenAdmin(body)))) reload()
  }
  return (
    <div className="mt-2 pl-3 border-l border-[var(--color-border)]" data-garden-data-grants={widget}>
      {admins.map((g) => (
        <div key={g.id} className="flex items-center justify-between gap-2 py-1" data-garden-data-admin={g.subjectName ?? g.subjectId}>
          <span className="text-xs text-[var(--color-text-primary)]">
            Grant admin: {g.subjectName ?? g.subjectId} · {g.level} · {g.prefix || 'everything'}
          </span>
          <button type="button" className={BTN} disabled={act.busy} onClick={() => void unadmin(g)}>
            Remove
          </button>
        </div>
      ))}
      {plain.length === 0 && admins.length === 0 ? <p className={MUTED}>No agent can read or write this data.</p> : null}
      {plain.map((g) => (
        <div key={g.id} className="flex items-center justify-between gap-2 py-1" data-garden-data-grant={subjectText(g)}>
          <span className="text-xs text-[var(--color-text-primary)]">
            {subjectText(g)} · {g.level} · {g.prefix || 'everything'}
            <span className={`${MUTED} ml-1`}>by {madeByText(g, rows)}</span>
            {g.subjectKind === 'project' ? (
              <span className={`${MUTED} ml-1`} title="Any agent can join a project, so this grant is a convenience, not a wall.">
                (convenience: any agent can join a project)
              </span>
            ) : null}
          </span>
          <button type="button" className={BTN} disabled={act.busy} onClick={() => revoke(g)}>
            Revoke
          </button>
        </div>
      ))}
      {act.error ? (
        <p className="text-[11px] text-[var(--color-status-error)] mt-1" data-testid="garden-data-grant-error">
          {act.error}
        </p>
      ) : null}
    </div>
  )
}

function DataLine({
  line,
  reload,
  accessOn,
}: {
  line: DataLineModel
  reload: () => void
  /** Garden access on (G28): no claim controls, grants under Advanced. */
  accessOn: boolean
}): React.JSX.Element {
  const { widget, space, madeBy } = line
  const [open, setOpen] = useState(false)
  const [detail, setDetail] = useState<GardenShowResponse | null>(null)
  const act = useAction()
  const rev = space?.rev ?? 0
  const grants = space?.grants ?? 0

  // Read the grant rows when they decide a button (an agent maker) or the
  // line is open. Re-read when the space's rev or grant count moves.
  const wantDetail = space !== null && (open || (madeBy?.kind === 'agent' && grants > 0))
  useEffect(() => {
    if (!wantDetail) return
    let live = true
    showGardenSpace(widget)
      .then((d) => live && setDetail(d))
      .catch((err: unknown) => live && act.run(() => Promise.reject(err)))
    return () => {
      live = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [widget, wantDetail, rev, grants])

  const maker = accessOn ? signerText(madeBy) : makerText(madeBy)
  const offerClaim = !accessOn && (madeBy?.kind === 'agent' || madeBy?.kind === 'unclaimed')
  const offerLet = !accessOn && madeBy?.kind === 'agent' && !makerHasGrant(madeBy.handle, detail, space)

  return (
    <div className="py-2 border-b border-[var(--color-border)]" data-garden-data-line={widget}>
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="text-xs text-[var(--color-text-primary)] truncate">
            {line.title && line.title !== widget ? `${line.title} (${widget})` : widget}
          </div>
          <div className={MUTED} data-garden-data-summary="">
            {space ? `${formatBytes(space.bytes)} · ${space.keys} key${space.keys === 1 ? '' : 's'}` : 'No data yet'}
            {space ? ` · ${grants} grant${grants === 1 ? '' : 's'}` : ''}
            {space && space.admins.length > 0 ? ` · grant admin: ${space.admins.join(', ')}` : ''}
            {maker ? ` · ${maker}` : ''}
          </div>
        </div>
        <div className="flex items-center gap-1 flex-shrink-0">
          {offerLet && madeBy?.kind === 'agent' ? (
            <button
              type="button"
              className={BTN}
              disabled={act.busy}
              onClick={() =>
                void act
                  .run(() => grantGardenData({ widget, agent: madeBy.handle, level: 'write', prefix: '' }), `${madeBy.handle} can use ${widget}'s data`)
                  .then((ok) => ok && reload())
              }
            >
              Let {madeBy.handle} use this data
            </button>
          ) : null}
          {offerClaim ? (
            <button
              type="button"
              className={BTN}
              disabled={act.busy}
              title="Sign this version as yours: the widget then reads and writes its data with your rights"
              onClick={() => void act.run(() => claimWidget(widget), `${widget} is yours`).then((ok) => ok && reload())}
            >
              It's mine
            </button>
          ) : null}
          {space ? (
            <>
              <button type="button" className={BTN} onClick={() => setOpen((o) => !o)} aria-expanded={open}>
                {accessOn ? (open ? 'Hide advanced' : 'Advanced') : open ? 'Hide grants' : 'Grants'}
              </button>
              <button type="button" className={BTN} disabled={act.busy} onClick={() => void act.run(() => exportAndSave(widget))}>
                Export
              </button>
              <button
                type="button"
                className={DANGER_BTN}
                disabled={act.busy}
                onClick={async () => {
                  if (!(await confirmDelete(widget, space.bytes))) return
                  if (await act.run(() => deleteGardenSpace(widget, widget), `${widget}'s data deleted (a snapshot was kept)`)) reload()
                }}
              >
                Delete
              </button>
            </>
          ) : null}
        </div>
      </div>
      {open && detail && accessOn ? (
        <div className="mt-2" data-garden-data-advanced={widget}>
          <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
            Advanced · data grants
          </div>
          <p className={MUTED}>
            Agents on a Garden&apos;s list already use the data of its widgets. These grants are for any other agent.
          </p>
          <GrantList widget={widget} detail={detail} reload={reload} />
        </div>
      ) : open && detail ? (
        <GrantList widget={widget} detail={detail} reload={reload} />
      ) : null}
      {act.error ? (
        <p className="text-[11px] text-[var(--color-status-error)] mt-1" data-testid="garden-data-line-error">
          {act.error}
        </p>
      ) : null}
    </div>
  )
}

function OrphanLine({ space, reload }: { space: GardenSpaceRow; reload: () => void }): React.JSX.Element {
  const act = useAction()
  return (
    <div className="flex items-center justify-between gap-3 py-1.5" data-garden-data-orphan={space.widget}>
      <span className="text-xs text-[var(--color-text-primary)]">
        {space.widget} <span className={MUTED}>({formatBytes(space.bytes)})</span>
      </span>
      <div className="flex items-center gap-1">
        <button type="button" className={BTN} disabled={act.busy} onClick={() => void act.run(() => exportAndSave(space.widget))}>
          Export
        </button>
        <button
          type="button"
          className={DANGER_BTN}
          disabled={act.busy}
          onClick={async () => {
            if (!(await confirmDelete(space.widget, space.bytes))) return
            if (await act.run(() => deleteGardenSpace(space.widget, space.widget))) reload()
          }}
        >
          Delete
        </button>
      </div>
      {act.error ? <p className="text-[11px] text-[var(--color-status-error)]">{act.error}</p> : null}
    </div>
  )
}

export interface ZenWidgetDataProps {
  /** A Garden's pane in Settings → Gardens: only these widgets' lines (the
   *  custom widgets placed in that Garden). No "Data with no widget". */
  only?: ReadonlySet<string> | null
  /** "Not in a Garden": every line whose widget is NOT in this set (placed
   *  nowhere), plus "Data with no widget". */
  except?: ReadonlySet<string> | null
  /** Inside a Settings → Gardens pane: a section header ("Local Data")
   *  instead of the page title. Absent: the whole page, as before. */
  embedded?: boolean
}

/** Which Data lines a pane shows (pure): `only` keeps those widgets,
 *  `except` drops them. A widget in two Gardens is in both `only` sets. */
export function filterDataLines(
  lines: readonly DataLineModel[],
  { only, except }: Pick<ZenWidgetDataProps, 'only' | 'except'>,
): DataLineModel[] {
  return lines.filter((l) => (!only || only.has(l.widget)) && (!except || !except.has(l.widget)))
}

export function ZenWidgetData({ only = null, except = null, embedded = false }: ZenWidgetDataProps = {}): React.JSX.Element {
  const [spaces, setSpaces] = useState<GardenSpacesResponse | null>(null)
  const [widgets, setWidgets] = useState<WidgetRow[]>([])
  const [error, setError] = useState<string | null>(null)
  const seq = useRef(0)
  const accessOn = useGardenAccessSwitchStore((s) => s.on) === true

  const load = useCallback(async (): Promise<void> => {
    const mine = ++seq.current
    try {
      const [s, w] = await Promise.all([
        listGardenSpaces(),
        daemonCliGet<{ widgets?: WidgetRow[] }>(zenLocalScope(), 'zen/widgets'),
      ])
      if (mine !== seq.current) return
      setSpaces(s)
      setWidgets(Array.isArray(w.widgets) ? w.widgets : [])
      setError(null)
    } catch (err) {
      if (mine === seq.current) setError(errText(err))
    }
  }, [])

  useEffect(() => {
    void load()
    let timer: ReturnType<typeof setTimeout> | null = null
    const soon = (): void => {
      if (timer) return
      timer = setTimeout(() => {
        timer = null
        void load()
      }, 250)
    }
    const offData = onGardenDataChanged(zenLocalScope(), soon)
    const offZen = watchLocalZenChanged(soon)
    return () => {
      if (timer) clearTimeout(timer)
      offData()
      offZen()
    }
  }, [load])

  const lines = spaces ? filterDataLines(buildDataLines(widgets, spaces), { only, except }) : []
  const orphans = spaces && !only ? spaces.spaces.filter((s) => s.orphanedAt != null) : []
  const reload = (): void => void load()

  return (
    <section data-settings-id="zen-gardens.data" data-garden-data="">
      {embedded ? (
        <>
          <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">Local Data</h3>
          <p className={`${MUTED} mt-1 mb-2`}>
            {only
              ? 'The data each widget in this Garden keeps on this computer. Agents on its list use it; any other agent needs a grant (under Advanced).'
              : 'Data of widgets no Garden places, and data whose widget is gone.'}
          </p>
        </>
      ) : (
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">{accessOn ? 'Widget data' : 'Local Data'}</h2>
      )}
      {embedded ? null : accessOn ? (
        <p className="text-xs text-[var(--color-text-muted)] mb-6">
          Each custom widget keeps its data on this computer. The agents on a Garden&apos;s list use the data of the widgets
          in it; any other agent needs a grant (under Advanced).
        </p>
      ) : (
        <p className="text-xs text-[var(--color-text-muted)] mb-6">
          Each custom widget keeps its data on this computer. An agent reads or writes it only with a grant you give (or one a
          grant admin you named gives).
        </p>
      )}
      {error ? (
        <div className="mb-2">
          <p className="text-[11px] text-[var(--color-status-error)]" data-testid="garden-data-load-error">
            {error}
          </p>
          <button type="button" className={`${BTN} mt-1`} onClick={reload}>
            Try again
          </button>
        </div>
      ) : null}
      {spaces === null && !error ? <p className={MUTED}>Loading…</p> : null}
      {spaces !== null && lines.length === 0 && (!embedded || orphans.length === 0) ? (
        <p className={MUTED}>{embedded ? (only ? 'No widget data in this Garden.' : 'Nothing here.') : 'No custom widgets yet.'}</p>
      ) : null}
      {lines.map((l) => (
        <DataLine key={l.widget} line={l} reload={reload} accessOn={accessOn} />
      ))}
      {orphans.length > 0 ? (
        <div className="mt-4" data-garden-data-orphans="">
          <div className="text-xs text-[var(--color-text-secondary)] mb-1">Data with no widget</div>
          {orphans.map((s) => (
            <OrphanLine key={s.widget} space={s} reload={reload} />
          ))}
          <p className={MUTED}>
            Its widget folder is gone or renamed. The data stays until you delete it; `k2 zen data move &lt;old&gt; &lt;new&gt;` gives it
            to a renamed widget.
          </p>
        </div>
      ) : null}
    </section>
  )
}
