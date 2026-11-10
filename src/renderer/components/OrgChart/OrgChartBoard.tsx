// The Org Chart's body: the toolbar, the canvas and the org view, over one
// chart from the daemon. Data rules (CLAUDE.md, daemon-first):
//   - the chart is read in effects and after writes, never in render;
//   - each gesture shows at once (optimistic) and is sent as one call; a
//     refusal puts the daemon's chart back and says its sentence;
//   - `org_chart_changed` (another person, an agent, the CLI) refetches;
//     while a write is in flight the refetch waits for it, so an older read
//     can't undo what the person just did;
//   - a refetch older than what we have (a lower revision) is dropped.
// View choices (the dotted layer, the minimap, the camera) are this
// viewer's own, kept in localStorage, never daemon state.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Button, Input, Toggle } from '@/components/ui'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { daemonErrorText } from '@/lib/daemon-error'
import { httpCodeOf } from '@/lib/daemon-http-error'
import type { OrgApi, OrgChart } from './org-api'
import { ORG_NAME_MAX } from './org-api'
import { layoutChart } from './org-layout'
import * as ops from './org-ops'
import { fitTo, zoomCentre, type Camera } from './org-camera'
import { OrgChartCanvas, type OrgGesture } from './OrgChartCanvas'
import { OrgMinimap } from './OrgMinimap'
import { OrgPanel } from './OrgPanel'

export const LS_ORG_CHART_DOTTED = 'k2.orgChart.dotted'
export const LS_ORG_CHART_MINIMAP = 'k2.orgChart.minimap'
export const DOTTED_LABEL = 'Show who can message whom'

function readPref(key: string, fallback: boolean): boolean {
  try {
    const v = typeof localStorage !== 'undefined' ? localStorage.getItem(key) : null
    return v === null ? fallback : v === '1'
  } catch {
    return fallback
  }
}

function writePref(key: string, on: boolean): void {
  try {
    if (typeof localStorage !== 'undefined') localStorage.setItem(key, on ? '1' : '0')
  } catch {
    // Blocked storage: the choice lasts for this window only.
  }
}

let pendingSeq = 0

export function OrgChartBoard({
  api,
  subscribe,
}: {
  api: OrgApi
  /** Calls `fn` on each `org_chart_changed {revision}`; returns the unsubscribe. */
  subscribe: (fn: (revision: number) => void) => () => void
}): React.JSX.Element {
  const [chart, setChart] = useState<OrgChart | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [openOrg, setOpenOrg] = useState<string | null>(null)
  const [selectedLine, setSelectedLine] = useState<string | null>(null)
  const [dotted, setDotted] = useState(() => readPref(LS_ORG_CHART_DOTTED, false))
  const [minimap, setMinimap] = useState(() => readPref(LS_ORG_CHART_MINIMAP, true))
  const [cam, setCam] = useState<Camera>({ x: 0, y: 0, s: 1 })
  const [viewport, setViewport] = useState({ w: 0, h: 0 })
  const [newName, setNewName] = useState<string | null>(null)

  const chartRef = useRef<OrgChart | null>(null)
  chartRef.current = chart
  const inFlight = useRef(0)
  const fitted = useRef(false)

  const load = useCallback(async (): Promise<void> => {
    // A write in flight reads the chart itself when it is done.
    if (inFlight.current > 0) return
    try {
      const next = await api.chart()
      // A write started while we read: its own reload follows; drop this one.
      if (inFlight.current > 0) return
      const cur = chartRef.current
      if (cur && next.revision < cur.revision) return
      setChart(next)
      setLoadError(null)
    } catch (e) {
      setLoadError(daemonErrorText(e))
    }
  }, [api])

  useEffect(() => {
    void load()
  }, [load])

  useEffect(
    () =>
      subscribe((revision) => {
        const cur = chartRef.current
        if (!cur || revision > cur.revision) void load()
      }),
    [subscribe, load],
  )

  const layout = useMemo(() => (chart ? layoutChart(chart) : null), [chart])

  // Fit the chart once it first has a size to fit into.
  useEffect(() => {
    if (fitted.current || !layout || viewport.w <= 0 || viewport.h <= 0) return
    fitted.current = true
    if (layout.bounds.w > 0) setCam(fitTo(layout.bounds, viewport.w, viewport.h, { max: 1, top: 0 }))
  }, [layout, viewport])

  /** Show `optimistic` now, send `call`; on a refusal say why. Then read the
   *  daemon's chart (the truth) once every write in flight is done. */
  const write = useCallback(
    async (optimistic: (c: OrgChart) => OrgChart, call: (revision: number) => Promise<void>): Promise<void> => {
      const before = chartRef.current
      if (!before) return
      const next = optimistic(before)
      chartRef.current = next
      setChart(next)
      setNotice(null)
      inFlight.current++
      try {
        try {
          await call(before.revision)
        } catch (e) {
          // A drag on a chart that changed underneath: read it again and
          // put the drag back on the new revision, once.
          if (httpCodeOf(e) !== 'stale_revision') throw e
          const fresh = await api.chart()
          chartRef.current = optimistic(fresh)
          setChart(chartRef.current)
          await call(fresh.revision)
        }
      } catch (e) {
        chartRef.current = before
        setChart(before)
        setNotice(daemonErrorText(e))
      } finally {
        inFlight.current--
      }
      if (inFlight.current === 0) await load()
    },
    [api, load],
  )

  const onGesture = useCallback(
    (g: OrgGesture): void => {
      switch (g.type) {
        case 'join': {
          if (g.from) {
            const c = chartRef.current
            const name = (id: string): string => c?.orgs.find((o) => o.id === id)?.name ?? id
            const agent = c?.agents.find((a) => a.id === g.agent)?.name ?? g.agent
            void useConfirmDialogStore
              .getState()
              .confirm({
                title: `Move ${agent} to ${name(g.org)}?`,
                message: `An agent is in one org at most, so ${agent} leaves ${name(g.from)}.`,
                confirmLabel: 'Move',
              })
              .then((yes) => {
                if (yes) void write((x) => ops.joinOrg(x, g.org, g.agent), () => api.moveMember(g.agent, g.org))
              })
            return
          }
          void write((x) => ops.joinOrg(x, g.org, g.agent), () => api.addMember(g.org, g.agent))
          return
        }
        case 'leave':
          void write((x) => ops.leaveOrg(x, g.agent), () => api.removeMember(g.org, g.agent))
          return
        case 'head':
          void write((x) => ops.makeHead(x, g.org, g.agent), () => api.setHead(g.org, g.agent))
          return
        case 'place':
          void write((x) => ops.place(x, g.node, g.x, g.y), (rev) => api.move(g.node, g.x, g.y, rev))
          return
        case 'line':
          void write((x) => ops.addLine(x, `pending-line-${++pendingSeq}`, g.kind, g.a, g.b), () => api.addLine(g.kind, g.a, g.b))
          return
        case 'unline':
          void write((x) => ops.removeLine(x, g.id), () => api.removeLine(g.id))
          return
      }
    },
    [api, write],
  )

  const tidyUp = (): void => {
    void write((x) => ops.tidy(x), (rev) => api.tidy(rev))
  }

  const createOrg = (): void => {
    const name = (newName ?? '').trim()
    if (!name) return
    setNewName(null)
    void write((x) => ops.createOrg(x, `pending-org-${++pendingSeq}`, name), () => api.create(name))
  }

  const deleteOrg = (orgId: string): void => {
    const org = chartRef.current?.orgs.find((o) => o.id === orgId)
    if (!org) return
    void useConfirmDialogStore
      .getState()
      .confirm({
        title: `Delete ${org.name}?`,
        message: 'Its agents leave it and its lines go. The agents themselves and their connections stay as they are.',
        confirmLabel: 'Delete org',
        destructive: true,
      })
      .then((yes) => {
        if (!yes) return
        setOpenOrg(null)
        void write((x) => ops.deleteOrg(x, orgId), () => api.deleteOrg(orgId))
      })
  }

  // Esc closes the org view.
  useEffect(() => {
    if (!openOrg) return
    const key = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setOpenOrg(null)
    }
    window.addEventListener('keydown', key)
    return () => window.removeEventListener('keydown', key)
  }, [openOrg])

  const agents = useMemo(() => new Map((chart?.agents ?? []).map((a) => [a.id, a])), [chart?.agents])
  const org = openOrg ? chart?.orgs.find((o) => o.id === openOrg) ?? null : null
  const hasPlaced = !!chart?.nodes.some((n) => n.placedBy === 'person')

  /** A panel edit: optimistic, then the daemon; a refusal is the panel's error. */
  const panelWrite = (optimistic: (c: OrgChart) => OrgChart, call: () => Promise<void>): Promise<void> =>
    new Promise<void>((resolve, reject) => {
      const before = chartRef.current
      if (!before) return resolve()
      chartRef.current = optimistic(before)
      setChart(chartRef.current)
      inFlight.current++
      call()
        .then(resolve, (e) => {
          chartRef.current = before
          setChart(before)
          reject(new Error(daemonErrorText(e)))
        })
        .finally(() => {
          inFlight.current--
          if (inFlight.current === 0) void load()
        })
    })

  if (loadError && !chart) {
    return (
      <div role="alert" className="flex flex-1 items-center justify-center p-6 type-row text-[var(--color-status-error-text)]">
        {loadError}
      </div>
    )
  }
  if (!chart || !layout) {
    return <div className="flex flex-1 items-center justify-center type-hint text-[var(--color-text-muted)]">Loading the Org Chart…</div>
  }

  const fit = (): void => {
    if (viewport.w > 0 && layout.bounds.w > 0) setCam(fitTo(layout.bounds, viewport.w, viewport.h, { max: 1, top: 0 }))
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="org-chart-board">
      <div className="flex flex-shrink-0 flex-wrap items-center gap-2 border-b border-[var(--color-border)] px-3 py-2">
        {chart.canEdit ? (
          newName === null ? (
            <Button size="sm" onClick={() => setNewName('')}>
              New org
            </Button>
          ) : (
            <form
              className="flex items-center gap-1.5"
              onSubmit={(e) => {
                e.preventDefault()
                createOrg()
              }}
            >
              <Input
                autoFocus
                value={newName}
                maxLength={ORG_NAME_MAX}
                placeholder="Org name"
                aria-label="New org name"
                onChange={(e) => setNewName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Escape') setNewName(null)
                }}
              />
              <Button size="sm" variant="accent" type="submit" disabled={!newName.trim()}>
                Create
              </Button>
              <Button size="sm" variant="quiet" type="button" onClick={() => setNewName(null)}>
                Cancel
              </Button>
            </form>
          )
        ) : null}
        {chart.canEdit ? (
          <Button
            size="sm"
            onClick={tidyUp}
            disabled={!hasPlaced}
            title={hasPlaced ? 'Put every card and bucket back where the chart lays them out' : 'Everything is already laid out automatically'}
          >
            Tidy up
          </Button>
        ) : (
          <span
            data-testid="org-chart-view-only"
            className="border border-[var(--color-border)] px-2 py-0.5 type-badge uppercase tracking-wider text-[var(--color-text-muted)] rounded-[var(--radius-pill)]"
            title="Only an Owner or Admin arranges the Org Chart."
          >
            View only
          </span>
        )}
        <label className="flex items-center gap-2 type-hint text-[var(--color-text-secondary)]">
          <Toggle
            size="sm"
            checked={dotted}
            aria-label={DOTTED_LABEL}
            data-testid="org-chart-dotted-toggle"
            onChange={() => {
              const next = !dotted
              setDotted(next)
              writePref(LS_ORG_CHART_DOTTED, next)
            }}
          />
          {DOTTED_LABEL}
        </label>
        {notice ? (
          <span role="status" data-testid="org-chart-notice" className="type-hint text-[var(--color-status-warn-text)]">
            {notice}
          </span>
        ) : null}
        <div className="ml-auto flex items-center gap-1">
          <Button size="sm" variant="quiet" aria-label="Zoom out" onClick={() => setCam((c) => zoomCentre(c, 0.8, viewport.w, viewport.h))}>
            −
          </Button>
          <span className="w-12 text-center type-hint text-[var(--color-text-muted)]">{Math.round(cam.s * 100)}%</span>
          <Button size="sm" variant="quiet" aria-label="Zoom in" onClick={() => setCam((c) => zoomCentre(c, 1.25, viewport.w, viewport.h))}>
            +
          </Button>
          <Button size="sm" variant="quiet" onClick={fit}>
            Fit
          </Button>
          <Button
            size="sm"
            variant={minimap ? 'default' : 'quiet'}
            aria-pressed={minimap}
            onClick={() => {
              setMinimap(!minimap)
              writePref(LS_ORG_CHART_MINIMAP, !minimap)
            }}
          >
            Minimap
          </Button>
        </div>
      </div>

      <div className="relative flex min-h-0 flex-1">
        <div className="relative min-w-0 flex-1">
          <OrgChartCanvas
            chart={chart}
            layout={layout}
            canEdit={chart.canEdit}
            dotted={dotted}
            cam={cam}
            onCamera={setCam}
            onGesture={onGesture}
            onRefuse={setNotice}
            onOpenOrg={setOpenOrg}
            selectedLine={selectedLine}
            onSelectLine={setSelectedLine}
            onViewport={setViewport}
          />
          {minimap ? (
            <div
              className="absolute bottom-3 right-3 z-40 overflow-hidden border border-[var(--color-border)] bg-[var(--color-bg-surface)] rounded-[var(--radius-box)]"
              data-material="frost"
            >
              <OrgMinimap layout={layout} cam={cam} viewport={viewport} onCamera={setCam} />
            </div>
          ) : null}
        </div>
        {org ? (
          <OrgPanel
            key={org.id}
            org={org}
            agents={agents}
            canEdit={chart.canEdit}
            onClose={() => setOpenOrg(null)}
            onRename={(name) => panelWrite((x) => ops.renameOrg(x, org.id, name), () => api.rename(org.id, name))}
            onPurpose={(purpose) => panelWrite((x) => ops.setPurpose(x, org.id, purpose), () => api.setPurpose(org.id, purpose))}
            onBulletin={(lines) =>
              panelWrite((x) => ops.setBulletin(x, org.id, lines, 'you', Date.now()), () => api.setBulletin(org.id, lines))
            }
            onHead={(agent) => onGesture({ type: 'head', org: org.id, agent })}
            onDelete={() => deleteOrg(org.id)}
          />
        ) : null}
      </div>
    </div>
  )
}

