// Settings → Gardens with Garden access ON (prd-garden-access-v1 §6,
// G25–G27; Rosson 2026-10-22: "auditable in one place"). Master-detail,
// laid out like Settings → Workspaces / Agents: a list column on the left
// (`w-72`) and the selected entry on the right (title, one line, then card
// groups of rows at one content width).
//
//   List (G25):     Gardens / one line.
//                   Styles                 ← the computer-wide look, first
//                   Your Gardens           ← lock glyph, "N editors", "made by X"; drag a row (Alt+↑/↓);
//                                            New Garden (the existing modal) is the group's action
//                   Deleted Gardens (n)    ← Restore and read-only History
//                   Not in a Garden        ← data and sources no Garden uses
//   Garden (G26):   History… · Rename · ⋯ (Delete); "Made by X · date";
//                   Agents who can edit (Remove with an 8 s Undo, Add agent
//                   through the A–Z picker); this Garden's settings (what it
//                   sees, Lock, Get K2's design updates); Widgets (last
//                   signer, History…); then (Rosson 2026-10-23) the Garden's
//                   Local Data and External Data.
//   History lives only in modals (G26a, `ZenGardenHistoryModal`); "not
//   signed" and Sign live only there too, never on this page.
//
// Thin client (daemon-first): every fact is read from this computer's daemon
// (`lib/zen/garden-access-api.ts`, `lib/zen/zen-settings.ts`) and every
// control posts one gesture. Re-read on open, on a Garden change
// (`zen_changed` re-reads the Garden list, and this page follows it), after
// each gesture and on window focus. No polling, no `fetchProjects()`.
// Every failed read shows the shared load card: what failed in plain words,
// the server's reason quietly, and Try again.
//
// Garden access is always on from 0.46.3 (Rosson 2026-10-23: no switch);
// `ZenGardensSection` shows this page once Gardens are set up.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { floatMaterial } from '@/components/ui/float-material'
import { revealItemInDir } from '@tauri-apps/plugin-opener'
import { SettingRow, SettingsGroup, SettingsLoadState, SETTING_ROW_HINT, SETTING_ROW_LABEL } from '../controls/SettingControls'
import { ZenGardenSyncControls, useZenSyncLive } from './ZenGardenSyncControls'
import { InlineError, NewGarden, ZEN_TOKENS_IN_SETTINGS, attempt, revealLabel } from './zen-gardens-ui'
import {
  checkZenGardenName,
  deleteZenSettingsGarden,
  loadZenSettings,
  reorderZenSettingsGarden,
  renameZenSettingsGardenNow,
  resetZenSettingsFile,
  setZenGlobalTheme,
  useZenSettingsStore,
  zenSettingsErrorText,
  type ZenSettingsGarden,
} from '@/lib/zen/zen-settings'
import { useZenSyncStore } from '@/lib/zen/zen-sync'
import {
  fetchDeletedGardens,
  fetchGardenAccess,
  fetchGardenHistory,
  fetchGardenListFacts,
  fetchGardenWidgets,
  fetchLocalAgents,
  gardenAccessErrorText,
  gardenDay,
  gardenEditorAddedWords,
  gardenHistoryLine,
  gardenWhen,
  gardenWidgetSignerWords,
  restoreDeletedGarden,
  setGardenEditor,
  gardenActorWords,
  type DeletedGarden,
  type GardenAccessList,
  type GardenHistoryItem,
  type GardenListFacts,
  type GardenWidgetLine,
  type LocalAgent,
} from '@/lib/zen/garden-access-api'
import { ZenGardenHistoryModal, type GardenHistoryTarget } from '@/components/Zen/widgets/ZenGardenHistoryModal'
import { SearchableAgentList, tokenAndMatch, type AgentListSection } from '@/components/ui/SearchableAgentList'
import { Button, Callout, Input, OverflowMenu, Surface, Tag } from '@/components/ui'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { useToastStore } from '@/stores/toast'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { primaryScope } from '@/kessel/server-scope'
import { buildGardenMembership, fetchGardenUses, type GardenMembership } from '@/lib/zen/garden-membership'
import { ZenWidgetData } from './ZenWidgetData'
import { DataSourcesSection } from './DataSourcesSection'
import { gardensLandingSelection, takeGardensLanding, useGardensLandingStore } from '../gardens-landing'

const CONTENT_WIDTH = 'max-w-[52rem]'
/** A list row: the Workspaces list's row (selected: the accent wash and bar).
 *  Keyboard focus is the list rows' ring (`data-k2-focus`), drawn inside the
 *  row so the card's edge never clips it, in every look. */
const LIST_ROW =
  'relative w-full flex items-center gap-2 text-left px-3 py-2 no-drag cursor-pointer select-none outline-none transition-colors'
/** A two-line list row: the name over one line of detail. */
const LIST_COL_ROW =
  'relative w-full flex flex-col items-start text-left px-3 py-2 no-drag cursor-pointer select-none outline-none transition-colors'
const ROW_ON = 'bg-[var(--color-accent)]/15'
const ROW_OFF = 'hover:bg-[var(--color-wash-1)]'

/** Press-and-move distance before a Garden row starts dragging; less is a click. */
export const DRAG_THRESHOLD_PX = 4
const GARDEN_REORDER_HELP_ID = 'gardens-side-reorder-help'
export const GARDEN_REORDER_HELP = 'Drag a Garden to reorder it, or press Alt+Up Arrow or Alt+Down Arrow to move it.'

/** The page's lead line: what a Garden is, in the words of the app. */
export const GARDENS_LEAD = 'The pages you see in Zen Mode, and who can change each one.'

/** "Sees" (G26, Rosson Q1): a Garden sees everything the person sees. */
export const GARDEN_SEES_TEXT = 'Everything you can see'

/** The last list entry: widget data and data sources no Garden uses
 *  (Rosson 2026-10-23: nothing may become unreachable). */
export const NOT_IN_A_GARDEN = 'Not in a Garden'

// ── Selection ───────────────────────────────────────────────────────

export type GardensSelection =
  | { kind: 'styles' }
  | { kind: 'garden'; id: string }
  | { kind: 'deleted'; id: string }
  | { kind: 'unplaced' }

function LockGlyph(): React.JSX.Element {
  return (
    <svg width="9" height="10" viewBox="0 0 9 10" aria-label="Locked" role="img" fill="currentColor">
      <rect x="0.5" y="4.5" width="8" height="5" rx="1" />
      <path d="M2.2 4.5V3a2.3 2.3 0 0 1 4.6 0v1.5" fill="none" stroke="currentColor" strokeWidth="1.1" />
    </svg>
  )
}

/** The list row's second line (G25): "N editors" and/or "made by X". */
export function gardenSidebarDetail(facts: GardenListFacts | undefined): string {
  if (!facts) return ''
  const parts: string[] = []
  if (facts.editors !== null) parts.push(`${facts.editors} editor${facts.editors === 1 ? '' : 's'}`)
  if (facts.madeBy) parts.push(`made by ${facts.madeBy.name}`)
  return parts.join(' · ')
}

/** The selected row's accent bar (the Workspaces list's). */
function SelectedBar(): React.JSX.Element {
  return <span aria-hidden="true" className="absolute left-0 top-0 bottom-0 w-[3px] bg-[var(--color-accent)]" />
}

/** A list group's heading, with the group's action on the right (as a
 *  Settings card group's). */
function ListHeading({ children, action }: { children: React.ReactNode; action?: React.ReactNode }): React.JSX.Element {
  return (
    <div className="flex items-center justify-between gap-2 min-h-7 mb-2">
      <h3 className="type-heading text-[var(--color-text-primary)]">{children}</h3>
      {action}
    </div>
  )
}

/** A pane's title and the one line under it. */
function PaneHeader({ title, hint, actions, testId }: { title: React.ReactNode; hint?: React.ReactNode; actions?: React.ReactNode; testId?: string }): React.JSX.Element {
  return (
    <header className="flex items-start justify-between gap-4 min-w-0">
      <div className="min-w-0 flex-1">
        <h2 className="type-title text-[var(--color-text-primary)] truncate" data-garden-pane-title={testId}>
          {title}
        </h2>
        {hint ? <p className={SETTING_ROW_HINT}>{hint}</p> : null}
      </div>
      {actions ? <div className="flex items-center gap-2 flex-shrink-0">{actions}</div> : null}
    </header>
  )
}

/** A failed gesture: what failed in plain words, the reason after it. */
function ActionError({ text, onDismiss, testId }: { text: string | null; onDismiss(): void; testId?: string }): React.JSX.Element | null {
  if (!text) return null
  return (
    <Callout tone="error" role="alert" data-testid={testId} className="flex items-start justify-between gap-4">
      <p className="type-hint min-w-0 break-words">{text}</p>
      <Button variant="quiet" className="flex-shrink-0" onClick={onDismiss}>
        Dismiss
      </Button>
    </Callout>
  )
}

// ── The list column ─────────────────────────────────────────────────

function Sidebar({
  gardens,
  facts,
  locked,
  deleted,
  sel,
  onSelect,
}: {
  gardens: readonly ZenSettingsGarden[]
  facts: Map<string, GardenListFacts>
  locked: (id: string) => boolean
  deleted: DeletedGarden[] | null
  sel: GardensSelection
  onSelect(s: GardensSelection): void
}): React.JSX.Element {
  const listRef = useRef<HTMLDivElement>(null)
  const [dragId, setDragId] = useState<string | null>(null)
  const [dropSlot, setDropSlot] = useState<number | null>(null)
  const [deletedOpen, setDeletedOpen] = useState(false)
  const pending = useRef(false)

  const move = (g: ZenSettingsGarden, to: number): void => {
    if (pending.current || to < 1 || to > gardens.length) return
    pending.current = true
    reorderZenSettingsGarden(g.id, to)
      .catch((err: unknown) =>
        useToastStore.getState().addToast(`Couldn’t move “${g.name}”: ${zenSettingsErrorText(err)}`, 'error'),
      )
      .finally(() => {
        pending.current = false
      })
  }

  // The whole Garden row drags (Rosson 2026-10-23: no handle). A plain click
  // selects; only a press that moves past DRAG_THRESHOLD_PX starts a drag,
  // and the click that follows that drag is swallowed. Same accent drop line
  // and one `garden/reorder` with the final spot, as the old grip.
  const justDragged = useRef(false)
  const refocusId = useRef<string | null>(null)
  useEffect(() => {
    const id = refocusId.current
    if (id === null || !listRef.current) return
    refocusId.current = null
    Array.from(listRef.current.querySelectorAll<HTMLElement>('[data-gardens-side="garden"]'))
      .find((el) => el.dataset.gardensSideId === id)
      ?.focus()
  }, [gardens])

  const onRowMouseDown = (e: React.MouseEvent, from: number): void => {
    if (e.button !== 0) return
    const g = gardens[from]
    const startX = e.clientX
    const startY = e.clientY
    let started = false
    let slot: number | null = null
    const prevCursor = document.body.style.cursor
    const onMoveEv = (ev: MouseEvent): void => {
      if (!started) {
        if (Math.hypot(ev.clientX - startX, ev.clientY - startY) <= DRAG_THRESHOLD_PX) return
        started = true
        setDragId(g.id)
        document.body.style.cursor = 'grabbing'
      }
      ev.preventDefault()
      if (!listRef.current) return
      const rows = listRef.current.querySelectorAll('[data-garden-side-row]')
      let idx = 0
      for (let i = 0; i < rows.length; i++) {
        const r = rows[i].getBoundingClientRect()
        if (ev.clientY > r.top + r.height / 2) idx = i + 1
      }
      slot = idx
      setDropSlot(idx)
    }
    const onUp = (): void => {
      document.removeEventListener('mousemove', onMoveEv)
      document.removeEventListener('mouseup', onUp)
      if (!started) return
      document.body.style.cursor = prevCursor
      // Swallow the click this release may fire on the row; a release off
      // the row fires none, so the flag clears on the next tick either way.
      justDragged.current = true
      setTimeout(() => {
        justDragged.current = false
      }, 0)
      setDragId(null)
      setDropSlot(null)
      if (slot === null || slot === from || slot === from + 1) return
      move(g, (slot > from ? slot - 1 : slot) + 1)
    }
    document.addEventListener('mousemove', onMoveEv)
    document.addEventListener('mouseup', onUp)
  }

  // Keyboard reorder on the focused row: Alt+↑ / Alt+↓ (plain arrows stay
  // free for the rest of the page). Focus follows the moved row.
  const onRowKeyDown = (e: React.KeyboardEvent, i: number): void => {
    if (!e.altKey || e.metaKey || e.ctrlKey || e.shiftKey) return
    if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return
    e.preventDefault()
    const to = i + 1 + (e.key === 'ArrowUp' ? -1 : 1)
    if (to < 1 || to > gardens.length) return
    refocusId.current = gardens[i].id
    move(gardens[i], to)
  }

  const stylesOn = sel.kind === 'styles'
  const unplacedOn = sel.kind === 'unplaced'
  return (
    <aside
      aria-label="Gardens"
      className="w-72 flex-shrink-0 min-h-0 flex flex-col border-r border-[var(--color-border)]"
      data-gardens-sidebar=""
    >
      <div className="flex-shrink-0 px-4 pt-6 pb-4 space-y-3">
        <div>
          <h2 className="type-title text-[var(--color-text-primary)]">Gardens</h2>
          <p className={SETTING_ROW_HINT}>{GARDENS_LEAD}</p>
        </div>
        <button
          type="button"
          data-gardens-side="styles"
          data-k2-chamfer="control"
          aria-label="Styles"
          aria-current={stylesOn ? 'page' : undefined}
          className={`relative w-full flex flex-col items-start px-3 py-2 rounded-[var(--radius-selector)] text-left no-drag cursor-pointer outline-none transition-colors k2-focus ${
            stylesOn ? 'bg-[var(--color-accent)]/15' : 'hover:bg-[var(--color-wash-1)]'
          }`}
          onClick={() => onSelect({ kind: 'styles' })}
        >
          <span className="type-name text-[var(--color-text-primary)]">Styles</span>
          <span className="type-hint text-[var(--color-text-secondary)]">The look of every Garden</span>
        </button>
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto px-4 pb-4 space-y-5 [scrollbar-gutter:stable]">
        <section>
          <ListHeading action={<NewGarden gardens={gardens} />}>Your Gardens</ListHeading>
          <p id={GARDEN_REORDER_HELP_ID} className="sr-only">
            {GARDEN_REORDER_HELP}
          </p>
          <Surface chamfer material="tile" className="overflow-hidden">
            <div ref={listRef} data-gardens-side-list="" className="divide-y divide-[var(--color-border)]">
              {gardens.map((g, i) => {
                const on = sel.kind === 'garden' && sel.id === g.id
                const detail = gardenSidebarDetail(facts.get(g.id))
                return (
                  <div
                    key={g.id}
                    data-garden-side-row={g.id}
                    className={`relative flex items-stretch ${dragId === g.id ? 'opacity-30' : ''}`}
                  >
                    {dropSlot === i && dragId !== null && (
                      <div data-garden-side-drop="top" className="absolute left-0 right-0 -top-px h-[2px] bg-[var(--color-accent)] z-10" />
                    )}
                    {dropSlot === gardens.length && i === gardens.length - 1 && dragId !== null && (
                      <div data-garden-side-drop="bottom" className="absolute left-0 right-0 -bottom-px h-[2px] bg-[var(--color-accent)] z-10" />
                    )}
                    <button
                      type="button"
                      data-gardens-side="garden"
                      data-gardens-side-id={g.id}
                      data-garden-draggable=""
                      data-k2-focus=""
                      aria-label={`Open ${g.name}`}
                      aria-current={on ? 'page' : undefined}
                      aria-describedby={GARDEN_REORDER_HELP_ID}
                      aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
                      onMouseDown={(e) => onRowMouseDown(e, i)}
                      onKeyDown={(e) => onRowKeyDown(e, i)}
                      className={`${LIST_COL_ROW} ${dragId === g.id ? 'cursor-grabbing' : ''} ${on ? ROW_ON : ROW_OFF}`}
                      onClick={() => {
                        if (justDragged.current) {
                          justDragged.current = false
                          return
                        }
                        onSelect({ kind: 'garden', id: g.id })
                      }}
                    >
                      {on && <SelectedBar />}
                      <span className="flex items-center gap-1.5 type-name text-[var(--color-text-primary)] min-w-0 max-w-full">
                        <span className="truncate">{g.name}</span>
                        {locked(g.id) ? (
                          <span className="text-[var(--color-text-secondary)]" data-garden-side-locked="">
                            <LockGlyph />
                          </span>
                        ) : null}
                      </span>
                      {detail ? (
                        <span className="block type-hint text-[var(--color-text-secondary)] truncate max-w-full" data-garden-side-detail="">
                          {detail}
                        </span>
                      ) : null}
                    </button>
                  </div>
                )
              })}
            </div>
          </Surface>
        </section>
        {deleted !== null && deleted.length > 0 ? (
          <section data-gardens-deleted="">
            <button
              type="button"
              aria-expanded={deletedOpen}
              data-gardens-deleted-toggle=""
              data-k2-focus=""
              className="relative flex items-center gap-1.5 min-h-7 -mx-2 px-2 mb-2 type-heading text-[var(--color-text-primary)] no-drag cursor-pointer outline-none"
              onClick={() => setDeletedOpen((o) => !o)}
            >
              <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.75} className={`w-3 h-3 transition-transform ${deletedOpen ? 'rotate-90' : ''}`}>
                <path d="M6 3.5 10.5 8 6 12.5" />
              </svg>
              Deleted Gardens ({deleted.length})
            </button>
            {deletedOpen && (
              <Surface chamfer material="tile" className="overflow-hidden">
                <div className="divide-y divide-[var(--color-border)]">
                  {deleted.map((d) => {
                    const on = sel.kind === 'deleted' && sel.id === d.id
                    return (
                      <button
                        key={d.id}
                        type="button"
                        data-gardens-side="deleted"
                        data-gardens-side-id={d.id}
                        data-k2-focus=""
                        aria-label={`Open deleted ${d.name}`}
                        className={`${LIST_ROW} type-row text-[var(--color-text-secondary)] ${on ? ROW_ON : ROW_OFF}`}
                        onClick={() => onSelect({ kind: 'deleted', id: d.id })}
                      >
                        {on && <SelectedBar />}
                        <span className="truncate">{d.name}</span>
                      </button>
                    )
                  })}
                </div>
              </Surface>
            )}
          </section>
        ) : null}
        <section>
          <ListHeading>Other data</ListHeading>
          <Surface chamfer material="tile" className="overflow-hidden">
            <button
              type="button"
              data-gardens-side="unplaced"
              data-k2-focus=""
              aria-label={NOT_IN_A_GARDEN}
              aria-current={unplacedOn ? 'page' : undefined}
              className={`${LIST_COL_ROW} ${unplacedOn ? ROW_ON : ROW_OFF}`}
              onClick={() => onSelect({ kind: 'unplaced' })}
            >
              {unplacedOn && <SelectedBar />}
              <span className="type-name text-[var(--color-text-primary)]">{NOT_IN_A_GARDEN}</span>
              <span className="type-hint text-[var(--color-text-secondary)]">Data and sources no Garden uses</span>
            </button>
          </Surface>
        </section>
      </div>
    </aside>
  )
}

// ── A Garden's data (Rosson 2026-10-23) ─────────────────────────────

/** Local Data and External Data for one list entry: a Garden (`garden`
 *  set: its widgets' data, the sources its cards read) or "Not in a Garden"
 *  (everything no Garden uses). The lists are the old pages' components,
 *  filtered; their actions are unchanged. */
function GardenDataSections({
  garden,
  membership,
  error,
  onRetry,
}: {
  garden: string | null
  membership: GardenMembership | null
  error: string | null
  onRetry(): void
}): React.JSX.Element {
  const onlySources = useMemo(() => {
    if (!membership) return null
    if (garden) {
      const mine = membership.sources.get(garden) ?? new Set<string>()
      return (name: string) => mine.has(name)
    }
    return (name: string) => !membership.usedSources.has(name)
  }, [membership, garden])
  if (error) {
    return (
      <div data-garden-data-sections-error="">
        <SettingsLoadState
          kind="error"
          title="K2 couldn’t work out which data belongs where"
          detail={error}
          testId="garden-membership-error"
          action={<Button variant="quiet" onClick={onRetry}>Try again</Button>}
        >
          Local Data and External Data show once K2 can read every Garden’s page.
        </SettingsLoadState>
      </div>
    )
  }
  if (!membership || !onlySources) return <SettingsLoadState kind="loading" title="Loading this data…" />
  return (
    <>
      <section data-garden-local-data={garden ?? 'unplaced'}>
        {garden ? (
          <ZenWidgetData embedded only={membership.widgets.get(garden) ?? new Set<string>()} />
        ) : (
          <ZenWidgetData embedded except={membership.placedWidgets} />
        )}
      </section>
      <section data-garden-external-data={garden ?? 'unplaced'}>
        <DataSourcesSection embedded only={onlySources} />
      </section>
    </>
  )
}

function UnplacedPane(props: { membership: GardenMembership | null; error: string | null; onRetry(): void }): React.JSX.Element {
  return (
    <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`} data-gardens-pane="unplaced">
      <PaneHeader
        title={NOT_IN_A_GARDEN}
        hint="Data and sources no Garden uses. Anything a Garden uses is under that Garden."
      />
      <GardenDataSections garden={null} {...props} />
    </div>
  )
}

// ── Styles (G25) ────────────────────────────────────────────────────

function StylesPane({ onHistory }: { onHistory(t: GardenHistoryTarget): void }): React.JSX.Element {
  const st = useZenSettingsStore()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [justUsed, setJustUsed] = useState<string | null>(null)
  const [justReset, setJustReset] = useState(false)
  const [revealError, setRevealError] = useState<string | null>(null)
  const [recent, setRecent] = useState<GardenHistoryItem[] | null>(null)
  const [recentError, setRecentError] = useState<string | null>(null)

  const loadRecent = useCallback(() => {
    setRecentError(null)
    fetchGardenHistory('*', { limit: 20 })
      .then((p) => setRecent(p.items))
      .catch((err: unknown) => setRecentError(gardenAccessErrorText(err)))
  }, [])
  // Re-read after a look change anywhere (the Garden list re-reads on every
  // `zen_changed`; the theme rides the same answer).
  useEffect(loadRecent, [loadRecent, st.globalTheme])
  useEffect(() => {
    if (!justUsed && !justReset) return
    const t = window.setTimeout(() => {
      setJustUsed(null)
      setJustReset(false)
    }, 2500)
    return () => window.clearTimeout(t)
  }, [justUsed, justReset])

  const use = async (name: string): Promise<void> => {
    if (await attempt(() => setZenGlobalTheme(name), setBusy, (e) => setError(e ? `K2 couldn’t switch to ${name}. ${e}` : null))) setJustUsed(name)
  }

  const reset = async (): Promise<void> => {
    const yes = await useConfirmDialogStore.getState().confirm({
      title: 'Reset the shared look?',
      message: 'Every change to the look your Gardens share goes back to K2’s defaults. Each Garden’s own page stays as it is. K2 keeps a copy in history first.',
      confirmLabel: 'Reset',
      destructive: true,
    })
    if (yes && (await attempt(resetZenSettingsFile, setBusy, (e) => setError(e ? `K2 couldn’t reset the shared look. ${e}` : null)))) {
      setJustReset(true)
      loadRecent()
    }
  }

  return (
    <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`} data-gardens-pane="styles" data-settings-id="zen-gardens.advanced">
      <PaneHeader title="Styles" hint="How your Gardens look on this computer." />
      <ActionError text={error} onDismiss={() => setError(null)} testId="gardens-styles-error" />
      <SettingsGroup title="Theme" hint="Every Garden uses this theme unless it picks its own." settingId="zen-gardens.theme" card>
        {st.themes.length === 0 ? (
          <div className="px-4 py-3">
            <p className={SETTING_ROW_LABEL}>No themes yet</p>
            <p className={SETTING_ROW_HINT}>K2’s themes show here once Gardens finish setting up.</p>
          </div>
        ) : (
          <ul data-gardens-theme-list="" className="divide-y divide-[var(--color-border)]">
            {st.themes.map((t) => {
              const inUse = t.name === st.globalTheme
              return (
                <li key={t.name} className="px-4 py-3 flex items-center justify-between gap-6 hover:bg-[var(--color-wash-1)] transition-colors">
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="type-name text-[var(--color-text-primary)]">{t.name}</span>
                      {inUse && <Tag tone="ok">In use</Tag>}
                      {!t.builtin && <Tag tone="muted">Yours</Tag>}
                      {justUsed === t.name && <DoneNote text="Switched" />}
                    </div>
                    <p className={SETTING_ROW_HINT}>{t.summary || (t.builtin ? 'Comes with K2.' : 'A theme you or your agents made.')}</p>
                  </div>
                  {!inUse && (
                    <Button variant="quiet" className="flex-shrink-0" disabled={busy} aria-label={`Use ${t.name}`} onClick={() => void use(t.name)}>
                      Use
                    </Button>
                  )}
                </li>
              )
            })}
          </ul>
        )}
      </SettingsGroup>
      <SettingsGroup title="Files" card>
        {st.path ? (
          <SettingRow
            label="Gardens folder"
            hint="Where K2 keeps each Garden’s page, your themes and their history."
            settingId="zen-gardens.folder"
            below={revealError ? <InlineError text={`K2 couldn’t open the folder. ${revealError}`} testId="gardens-reveal-error" /> : undefined}
          >
            <Button
              variant="quiet"
              title={st.path}
              data-zen-gardens-path={st.path}
              onClick={() => {
                setRevealError(null)
                revealItemInDir(st.path as string).catch((err: unknown) => setRevealError(err instanceof Error ? err.message : String(err)))
              }}
            >
              {revealLabel()}
            </Button>
          </SettingRow>
        ) : null}
        <SettingRow
          label={
            <span className="inline-flex items-center gap-2">
              Shared look
              {justReset && <DoneNote text="Reset" />}
            </span>
          }
          hint="Undo every change to the look your Gardens share. K2 keeps a copy first."
        >
          <Button variant="danger-quiet" disabled={busy} data-gardens-reset-zen="" onClick={() => void reset()}>
            Reset…
          </Button>
        </SettingRow>
      </SettingsGroup>
      <section data-gardens-look-history="">
        {recentError ? (
          <div className="space-y-2">
            <h3 className="type-heading text-[var(--color-text-primary)] min-h-[1.5rem] flex items-center">Recent look changes</h3>
            <SettingsLoadState
              kind="error"
              title="K2 couldn’t read the recent look changes"
              detail={recentError}
              testId="gardens-look-history-error"
              action={<Button variant="quiet" onClick={loadRecent}>Try again</Button>}
            >
              Your Gardens look as they did. Try again in a moment.
            </SettingsLoadState>
          </div>
        ) : (
          <SettingsGroup
            title="Recent look changes"
            hint="Who changed the theme or reset the shared look, newest first."
            card
            action={
              <Button variant="quiet" onClick={() => onHistory({ kind: 'garden', garden: '*', name: 'Styles' })}>
                History…
              </Button>
            }
          >
            {recent === null ? (
              <div className="px-4 py-3">
                <p className={SETTING_ROW_HINT}>Loading…</p>
              </div>
            ) : recent.length === 0 ? (
              <div className="px-4 py-3">
                <p className={SETTING_ROW_LABEL}>No changes yet</p>
                <p className={SETTING_ROW_HINT}>A change to a theme shows here.</p>
              </div>
            ) : (
              recent.slice(0, 6).map((it) => (
                <div key={it.id} className="px-4 py-3" data-gardens-look-line={it.id}>
                  <p className={SETTING_ROW_LABEL}>{gardenHistoryLine(it)}</p>
                  <p className={SETTING_ROW_HINT}>{gardenWhen(it.at)}</p>
                </div>
              ))
            )}
          </SettingsGroup>
        )}
      </section>
    </div>
  )
}

/** A check and a word, for a moment, after a change lands. */
function DoneNote({ text }: { text: string }): React.JSX.Element {
  return (
    <span role="status" className="inline-flex items-center gap-1 type-hint text-[var(--color-status-ok-soft)] whitespace-nowrap">
      <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3">
        <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
      </svg>
      {text}
    </span>
  )
}

// ── Add agent picker (G26) ──────────────────────────────────────────

/** The picker's one section (pure): this computer's agents A–Z; a listed
 *  agent is checked (already on the list). */
export function gardenAddAgentSections(agents: readonly LocalAgent[], listed: ReadonlySet<string>): AgentListSection[] {
  const rows = [...agents]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((a) => ({
      value: a.id,
      label: a.name,
      keywords: [a.path],
      avatar: {
        kind: 'workspace' as const,
        path: a.path,
        name: a.name,
        color: a.color,
        id: a.id,
        iconUrl: a.iconUrl,
        fetchIcon: false,
      },
      state: listed.has(a.id) ? ('checked' as const) : ('pickable' as const),
      title: listed.has(a.id) ? `${a.name} can already edit this Garden` : undefined,
    }))
  return [{ key: 'local', label: null, rows }]
}

function AddAgent({
  listed,
  onPick,
  disabled,
}: {
  listed: ReadonlySet<string>
  onPick(agent: LocalAgent): void
  disabled: boolean
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [agents, setAgents] = useState<LocalAgent[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const menu = useAnchoredMenu<HTMLButtonElement>({ open, onClose: () => setOpen(false), gap: 4, width: 'min', minWidth: 280, align: 'end' })

  // Read this computer's agents when the picker opens (a gesture).
  useEffect(() => {
    if (!open) return
    let live = true
    setError(null)
    fetchLocalAgents()
      .then((a) => live && setAgents(a))
      .catch((err: unknown) => live && setError(gardenAccessErrorText(err)))
    return () => {
      live = false
    }
  }, [open])

  const sections = useMemo(() => gardenAddAgentSections(agents ?? [], listed), [agents, listed])
  return (
    <>
      <Button
        ref={menu.anchorRef}
        variant="quiet"
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        data-garden-add-agent=""
        onClick={() => {
          setQuery('')
          setOpen((o) => !o)
        }}
      >
        Add agent
      </Button>
      {open &&
        menu.portal(
          <div
            ref={menu.menuRef}
            data-garden-add-agent-picker=""
            {...floatMaterial('menu')}
            className="no-drag border border-[var(--color-border)] bg-[var(--color-bg-elevated)] p-2"
            style={menu.style}
          >
            {error ? (
              <p role="alert" className="type-hint text-[var(--color-status-error-soft)] p-1">
                K2 couldn’t list this computer’s agents. {error}
              </p>
            ) : agents === null ? (
              <p className={`${SETTING_ROW_HINT} p-1`}>Loading…</p>
            ) : (
              <SearchableAgentList
                sections={sections}
                query={query}
                onQuery={setQuery}
                matches={tokenAndMatch}
                onPick={(id) => {
                  const a = agents.find((x) => x.id === id)
                  if (!a) return
                  setOpen(false)
                  onPick(a)
                }}
                onEscape={() => setOpen(false)}
                placeholder="Search agents"
                emptyText="No agents on this computer match."
                ariaLabel="Agents on this computer"
                autoFocus
                listClassName="max-h-64 overflow-y-auto"
              />
            )}
          </div>,
        )}
    </>
  )
}

// ── Garden pane (G26) ───────────────────────────────────────────────

function GardenPane({
  garden,
  gardens,
  onHistory,
  onDeleted,
  membership,
  membershipError,
  onMembershipRetry,
}: {
  garden: ZenSettingsGarden
  gardens: readonly ZenSettingsGarden[]
  onHistory(t: GardenHistoryTarget): void
  onDeleted(): void
  membership: GardenMembership | null
  membershipError: string | null
  onMembershipRetry(): void
}): React.JSX.Element {
  const [access, setAccess] = useState<GardenAccessList | null>(null)
  const [widgets, setWidgets] = useState<GardenWidgetLine[] | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [renaming, setRenaming] = useState(false)
  const [renamed, setRenamed] = useState(false)
  const [draft, setDraft] = useState(garden.name)
  const seq = useRef(0)
  const renameRef = useRef<HTMLButtonElement>(null)
  const id = garden.id

  const load = useCallback(() => {
    const mine = ++seq.current
    Promise.all([fetchGardenAccess(id), fetchGardenWidgets()])
      .then(([a, w]) => {
        if (mine !== seq.current) return
        setAccess(a)
        setWidgets(w.filter((x) => !x.builtin && x.gardens.includes(id)))
        setLoadError(null)
      })
      .catch((err: unknown) => {
        if (mine === seq.current) setLoadError(gardenAccessErrorText(err))
      })
  }, [id])

  // On open, on a Garden list change (`zen_changed`), and on window focus.
  useEffect(load, [load, gardens])
  useEffect(() => {
    window.addEventListener('focus', load)
    return () => window.removeEventListener('focus', load)
  }, [load])
  useEffect(() => {
    setRenaming(false)
    setDraft(garden.name)
  }, [id, garden.name])
  useEffect(() => {
    if (!renamed) return
    const t = window.setTimeout(() => setRenamed(false), 2500)
    return () => window.clearTimeout(t)
  }, [renamed])

  const run = async (what: string, fn: () => Promise<unknown>): Promise<boolean> => {
    setBusy(true)
    setError(null)
    try {
      await fn()
      return true
    } catch (err) {
      setError(`${what} ${gardenAccessErrorText(err)}`)
      return false
    } finally {
      setBusy(false)
      load()
    }
  }

  const add = (a: LocalAgent): void => {
    void run(`K2 couldn’t let ${a.name} edit ${garden.name}.`, () => setGardenEditor(id, a.id, true)).then((ok) => {
      if (ok) useToastStore.getState().addToast(`${a.name} can edit ${garden.name}`, 'success')
    })
  }

  // G26: Remove with an 8 s Undo toast (ZenGardenSyncControls' toast).
  const remove = (agent: string, name: string): void => {
    void run(`K2 couldn’t remove ${name}.`, () => setGardenEditor(id, agent, false)).then((ok) => {
      if (!ok) return
      useToastStore.getState().addToast(`${name} can no longer edit ${garden.name}`, 'success', 8000, {
        label: 'Undo',
        onClick: () =>
          void setGardenEditor(id, agent, true)
            .catch((err: unknown) => setError(`K2 couldn’t add ${name} back. ${gardenAccessErrorText(err)}`))
            .finally(load),
      })
    })
  }

  const stopRenaming = (): void => {
    setRenaming(false)
    setDraft(garden.name)
    requestAnimationFrame(() => renameRef.current?.focus())
  }

  const saveRename = async (): Promise<void> => {
    let clean: string
    try {
      clean = checkZenGardenName(draft, gardens, id)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      return
    }
    if (clean === garden.name) {
      stopRenaming()
      return
    }
    if (await attempt(() => renameZenSettingsGardenNow(id, clean), setBusy, (e) => setError(e ? `K2 couldn’t rename ${garden.name}. ${e}` : null))) {
      setRenaming(false)
      setRenamed(true)
    }
  }

  const askDelete = async (): Promise<void> => {
    const yes = await useConfirmDialogStore.getState().confirm({
      title: `Delete ${garden.name}?`,
      message: 'It leaves your Gardens. Its page moves to history in the Gardens folder, and you can restore it from Deleted Gardens.',
      confirmLabel: 'Delete',
      destructive: true,
    })
    if (yes && (await attempt(() => deleteZenSettingsGarden(id), setBusy, (e) => setError(e ? `K2 couldn’t delete ${garden.name}. ${e}` : null)))) onDeleted()
  }

  const listed = useMemo(() => new Set((access?.editors ?? []).map((e) => e.agent)), [access])
  const made = access?.made ? `Made by ${access.made.name}${access.made.at !== null ? ` · ${gardenDay(access.made.at)}` : ''}` : null
  return (
    <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`} data-gardens-pane="garden" data-garden-pane={id}>
      <div className="space-y-3">
        {renaming ? (
          <div className="flex items-center gap-2" role="group" aria-label={`Rename ${garden.name}`}>
            <Input
              autoFocus
              aria-label={`Name of ${garden.name}`}
              data-garden-pane-name=""
              value={draft}
              maxLength={80}
              disabled={busy}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault()
                  void saveRename()
                } else if (e.key === 'Escape') {
                  e.preventDefault()
                  e.stopPropagation()
                  stopRenaming()
                }
              }}
              className="flex-1 min-w-0 type-control"
            />
            <Button variant="quiet" onClick={stopRenaming}>
              Cancel
            </Button>
            <Button variant="accent" disabled={busy || draft.trim() === ''} onClick={() => void saveRename()}>
              Save
            </Button>
          </div>
        ) : (
          <PaneHeader
            title={
              <span className="inline-flex items-center gap-2 max-w-full">
                <span className="truncate">{garden.name}</span>
                {renamed && <DoneNote text="Renamed" />}
              </span>
            }
            hint={<span data-garden-pane-made="">{made ?? 'A Garden on this computer.'}</span>}
            testId=""
            actions={
              <>
                <Button variant="quiet" data-garden-pane-history="" onClick={() => onHistory({ kind: 'garden', garden: id, name: garden.name })}>
                  History…
                </Button>
                <Button ref={renameRef} variant="quiet" data-garden-pane-rename="" onClick={() => setRenaming(true)}>
                  Rename
                </Button>
                <OverflowMenu
                  label={`More for ${garden.name}`}
                  triggerProps={{ 'data-garden-pane-more': '' }}
                  items={[{ id: 'delete', label: 'Delete…', onSelect: () => void askDelete(), danger: true }]}
                />
              </>
            }
          />
        )}
        <ActionError text={error} onDismiss={() => setError(null)} testId="garden-pane-error" />
      </div>

      {loadError ? (
        <SettingsLoadState
          kind="error"
          title={`K2 couldn’t read who can edit ${garden.name}`}
          detail={loadError}
          testId="garden-pane-load-error"
          action={<Button variant="quiet" onClick={load}>Try again</Button>}
        >
          Nothing changed. Try again in a moment.
        </SettingsLoadState>
      ) : access === null ? (
        <SettingsLoadState kind="loading" title={`Loading ${garden.name}…`} />
      ) : (
        <div data-garden-editors="">
          <SettingsGroup
            title="Agents who can edit"
            hint="These agents can change this Garden’s page and widgets. Anyone else asks first."
            card
            action={<AddAgent listed={listed} onPick={add} disabled={busy} />}
          >
            {access.editors.length === 0 ? (
              <div className="px-4 py-3" data-garden-editors-empty="">
                <p className={SETTING_ROW_LABEL}>No agents yet</p>
                <p className={SETTING_ROW_HINT}>An agent that asks gets a card on its Thread with you.</p>
              </div>
            ) : (
              <ul className="divide-y divide-[var(--color-border)]">
                {access.editors.map((e) => (
                  <li
                    key={e.agent}
                    data-garden-editor={e.agent}
                    className="px-4 py-3 flex items-center gap-6 hover:bg-[var(--color-wash-1)] transition-colors"
                  >
                    <div className="flex-1 min-w-0">
                      <p className="flex items-center gap-2 min-w-0 type-name text-[var(--color-text-primary)]">
                        <span className="truncate">{e.name}</span>
                        <span
                          aria-hidden="true"
                          data-garden-editor-dot=""
                          className="w-2 h-2 rounded-[var(--radius-pill)] flex-shrink-0"
                          style={{ background: e.color ?? 'var(--color-text-muted)' }}
                        />
                      </p>
                      <p className={SETTING_ROW_HINT} data-garden-editor-added="">
                        {gardenEditorAddedWords(e)[0].toUpperCase() + gardenEditorAddedWords(e).slice(1)}
                      </p>
                    </div>
                    <Button
                      variant="quiet"
                      className="flex-shrink-0"
                      disabled={busy}
                      aria-label={`Remove ${e.name}`}
                      data-garden-editor-remove={e.agent}
                      onClick={() => remove(e.agent, e.name)}
                    >
                      Remove
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </SettingsGroup>
        </div>
      )}

      <section data-garden-pane-controls="">
        <SettingsGroup title="This Garden" card>
          <SettingRow label="What it can see" hint={`${GARDEN_SEES_TEXT}. Its widgets read what you can read on this computer.`} testId="garden-sees">
            {null}
          </SettingRow>
          <ZenGardenSyncControls garden={garden} variant="pane" />
        </SettingsGroup>
      </section>

      {loadError ? null : (
        <section data-garden-widgets="">
          <SettingsGroup title="Widgets" hint="Widgets made for this Garden, and who last signed off on each." card>
            {widgets === null ? (
              <div className="px-4 py-3">
                <p className={SETTING_ROW_HINT}>Loading…</p>
              </div>
            ) : widgets.length === 0 ? (
              <div className="px-4 py-3">
                <p className={SETTING_ROW_LABEL}>No widgets yet</p>
                <p className={SETTING_ROW_HINT}>A widget an agent makes for this Garden shows here.</p>
              </div>
            ) : (
              <ul className="divide-y divide-[var(--color-border)]">
                {widgets.map((w) => {
                  const signer = gardenWidgetSignerWords(w)
                  return (
                    <li
                      key={w.widget}
                      data-garden-widget={w.widget}
                      className="px-4 py-3 flex items-center justify-between gap-6 hover:bg-[var(--color-wash-1)] transition-colors"
                    >
                      <div className="flex-1 min-w-0">
                        <p className="type-name text-[var(--color-text-primary)] truncate">{w.title ?? w.widget}</p>
                        <p className={SETTING_ROW_HINT} data-garden-widget-signer="">
                          {signer ? signer[0].toUpperCase() + signer.slice(1) : w.widget}
                        </p>
                      </div>
                      <Button
                        variant="quiet"
                        className="flex-shrink-0"
                        data-garden-widget-history={w.widget}
                        aria-label={`History of ${w.title ?? w.widget}`}
                        onClick={() => onHistory({ kind: 'widget', widget: w.widget, garden: id })}
                      >
                        History…
                      </Button>
                    </li>
                  )
                })}
              </ul>
            )}
          </SettingsGroup>
        </section>
      )}

      <GardenDataSections garden={id} membership={membership} error={membershipError} onRetry={onMembershipRetry} />
    </div>
  )
}

// ── Deleted Garden pane (G25) ───────────────────────────────────────

function DeletedPane({
  garden,
  onHistory,
  onRestored,
}: {
  garden: DeletedGarden
  onHistory(t: GardenHistoryTarget): void
  onRestored(): void
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const who = garden.deletedBy ? gardenActorWords(garden.deletedBy).replace(/^You$/, 'you') : null
  return (
    <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`} data-gardens-pane="deleted" data-garden-deleted-pane={garden.id}>
      <PaneHeader
        title={garden.name}
        hint={`Deleted${who ? ` by ${who}` : ''}${garden.deletedAt !== null ? ` · ${gardenDay(garden.deletedAt)}` : ''}. K2 keeps a copy in history.`}
      />
      <ActionError text={error} onDismiss={() => setError(null)} testId="garden-restore-error" />
      <SettingsGroup title="This deleted Garden" card>
        <SettingRow label="Restore" hint="Puts it back in your Gardens as it was when it was deleted.">
          <Button
            variant="accent"
            disabled={busy}
            data-garden-restore=""
            onClick={() => {
              setBusy(true)
              setError(null)
              restoreDeletedGarden(garden.id)
                .then(onRestored)
                .catch((err: unknown) => setError(`K2 couldn’t restore ${garden.name}. ${gardenAccessErrorText(err)}`))
                .finally(() => setBusy(false))
            }}
          >
            {busy ? 'Restoring…' : 'Restore'}
          </Button>
        </SettingRow>
        <SettingRow label="History" hint="Every change to it before it was deleted. Read only.">
          <Button
            variant="quiet"
            data-garden-deleted-history=""
            onClick={() => onHistory({ kind: 'garden', garden: garden.id, name: garden.name, readOnly: true })}
          >
            History…
          </Button>
        </SettingRow>
      </SettingsGroup>
    </div>
  )
}

// ── The page ────────────────────────────────────────────────────────

export function ZenGardensAccessPage(): React.JSX.Element {
  const st = useZenSettingsStore()
  // The list's lock glyphs read the sync store; keep it live here too
  // (the pane's switches aren't mounted while Styles is open).
  useZenSyncLive()
  const sync = useZenSyncStore((s) => s.list)
  const [sel, setSel] = useState<GardensSelection>({ kind: 'styles' })
  const [facts, setFacts] = useState<Map<string, GardenListFacts>>(new Map())
  const [deleted, setDeleted] = useState<DeletedGarden[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [history, setHistory] = useState<GardenHistoryTarget | null>(null)

  // The list's facts and Deleted Gardens: re-read with the Garden list
  // (it re-reads on every local `zen_changed`) and on window focus.
  const loadSide = useCallback(() => {
    setError(null)
    fetchGardenListFacts()
      .then(setFacts)
      .catch((err: unknown) => setError(gardenAccessErrorText(err)))
    fetchDeletedGardens()
      .then(setDeleted)
      .catch((err: unknown) => setError(gardenAccessErrorText(err)))
  }, [])
  useEffect(loadSide, [loadSide, st.gardens])
  useEffect(() => {
    window.addEventListener('focus', loadSide)
    return () => window.removeEventListener('focus', loadSide)
  }, [loadSide])

  // A selected Garden that's gone (deleted here or elsewhere) → Styles.
  const selected = sel.kind === 'garden' ? (st.gardens.find((g) => g.id === sel.id) ?? null) : null
  const selectedDeleted = sel.kind === 'deleted' ? ((deleted ?? []).find((g) => g.id === sel.id) ?? null) : null
  useEffect(() => {
    if ((sel.kind === 'garden' && !selected) || (sel.kind === 'deleted' && deleted !== null && !selectedDeleted)) {
      setSel({ kind: 'styles' })
    }
  }, [sel, selected, selectedDeleted, deleted])

  const locked = (id: string): boolean => sync?.gardens.find((g) => g.id === id)?.locked === true

  // Which widgets and sources each Garden uses (one `zen/get` per Garden),
  // re-read with the Garden list (every local `zen_changed`) and on focus.
  const [membership, setMembership] = useState<GardenMembership | null>(null)
  const [membershipError, setMembershipError] = useState<string | null>(null)
  const memberSeq = useRef(0)
  const loadMembership = useCallback(() => {
    const mine = ++memberSeq.current
    const ids = st.gardens.map((g) => g.id)
    fetchGardenUses(ids)
      .then((uses) => {
        if (mine !== memberSeq.current) return
        setMembership(buildGardenMembership(uses, primaryScope().hostKey))
        setMembershipError(null)
      })
      .catch((err: unknown) => {
        if (mine === memberSeq.current) setMembershipError(gardenAccessErrorText(err))
      })
  }, [st.gardens])
  useEffect(loadMembership, [loadMembership])
  useEffect(() => {
    window.addEventListener('focus', loadMembership)
    return () => window.removeEventListener('focus', loadMembership)
  }, [loadMembership])

  // A deep link or search result for Local Data / External Data (or one
  // Garden): open its Garden, else "Not in a Garden" (gardens-landing.ts).
  const landing = useGardensLandingStore((s) => s.landing)
  const gardenIds = useMemo(() => st.gardens.map((g) => g.id), [st.gardens])
  useEffect(() => {
    if (!landing) return
    if (landing.garden && !gardenIds.includes(landing.garden) && st.gardens.length === 0) return // list not read yet
    const l = takeGardensLanding()
    const to = l ? gardensLandingSelection(l, gardenIds) : null
    if (to) setSel(to)
  }, [landing, gardenIds, st.gardens.length])

  return (
    <div className="flex h-full min-h-0" data-zen-gardens-section="" data-gardens-master-detail="">
      <Sidebar gardens={st.gardens} facts={facts} locked={locked} deleted={deleted} sel={sel} onSelect={setSel} />
      <div className="flex-1 min-w-0 min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]">
        {error || st.failure ? (
          <div className={`w-full ${CONTENT_WIDTH} mb-6`} data-gardens-error="">
            <SettingsLoadState
              kind="error"
              title={st.failure ? 'K2 couldn’t refresh the Gardens list' : 'K2 couldn’t read every detail of your Gardens'}
              detail={st.failure?.kind === 'unreadable' ? null : (st.failure ? zenSettingsErrorText(new Error(st.failure.message)) : error)}
              action={
                <Button
                  variant="quiet"
                  onClick={() => {
                    if (st.failure) void loadZenSettings()
                    loadSide()
                  }}
                >
                  Try again
                </Button>
              }
            >
              {st.failure ? 'What you see may be out of date.' : 'Who can edit each Garden, and Deleted Gardens, may be missing.'}
            </SettingsLoadState>
          </div>
        ) : null}
        {sel.kind === 'styles' ? <StylesPane onHistory={setHistory} /> : null}
        {selected ? (
          <GardenPane
            key={selected.id}
            garden={selected}
            gardens={st.gardens}
            onHistory={setHistory}
            onDeleted={() => setSel({ kind: 'styles' })}
            membership={membership}
            membershipError={membershipError}
            onMembershipRetry={loadMembership}
          />
        ) : null}
        {sel.kind === 'unplaced' ? <UnplacedPane membership={membership} error={membershipError} onRetry={loadMembership} /> : null}
        {selectedDeleted ? (
          <DeletedPane
            garden={selectedDeleted}
            onHistory={setHistory}
            onRestored={() => {
              const id = selectedDeleted.id
              void loadZenSettings().then(() => {
                setSel({ kind: 'garden', id })
                loadSide()
              })
            }}
          />
        ) : null}
      </div>
      {history ? <ZenGardenHistoryModal target={history} vars={ZEN_TOKENS_IN_SETTINGS} onClose={() => setHistory(null)} /> : null}
    </div>
  )
}
