// Settings → Gardens with Garden access ON (prd-garden-access-v1 §6,
// G25–G27; Rosson 2026-10-22: "auditable in one place"). Master-detail,
// modelled on Settings → Apps (`SkinAccessSection`): a `w-60` sidebar and a
// right pane (`flex-1 overflow-y-auto p-6`, `max-w-2xl` content).
//
//   Sidebar (G25):  Gardens / Your Zen pages on this computer.
//                   Styles                 ← the computer-wide look, first
//                   each Garden            ← lock glyph, "N editors", "made by X"; drag to reorder
//                   + New Garden           ← the existing modal
//                   ▸ Deleted Gardens (n)  ← Restore and read-only History
//   Garden pane (G26): History… · Rename · ⋯ (Delete); "Made by X · date";
//                   Agents who can edit (Remove with an 8 s Undo, + Add agent
//                   through the A–Z picker); "Sees: Everything you can see";
//                   Lock; Get K2's design updates; Widgets (last signer, History…);
//                   then (Rosson 2026-10-23) the Garden's Local Data (its
//                   placed widgets' data) and External Data (the sources its
//                   cards read): the old pages' lists, filtered, same actions.
//   Last rail entry, "Not in a Garden": data and sources no Garden uses, so
//   nothing is unreachable now that Local Data and External Data aren't
//   Settings pages of their own (`gardens-landing.ts` routes their links).
//   History lives only in modals (G26a, `ZenGardenHistoryModal`); "not
//   signed" and Sign live only there too, never on this page.
//
// Thin client (daemon-first): every fact is read from this computer's daemon
// (`lib/zen/garden-access-api.ts`, `lib/zen/zen-settings.ts`) and every
// control posts one gesture. Re-read on open, on a Garden change
// (`zen_changed` re-reads the Garden list, and this page follows it), after
// each gesture and on window focus. No polling, no `fetchProjects()`.
//
// Garden access is always on from 0.46.3 (Rosson 2026-10-23: no switch);
// `ZenGardensSection` shows this page once Gardens are set up. The old page
// stays only for tests of the retired path until 0.47.0.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { revealItemInDir } from '@tauri-apps/plugin-opener'
import { SettingDropdown } from '../controls/SettingControls'
import { ZenGardenSyncControls, useZenSyncLive } from './ZenGardenSyncControls'
import { BTN, INPUT, InlineError, NewGarden, ZEN_TOKENS_IN_SETTINGS, attempt, revealLabel, themeOptions } from './zen-gardens-ui'
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
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { useToastStore } from '@/stores/toast'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { primaryScope } from '@/kessel/server-scope'
import { buildGardenMembership, fetchGardenUses, type GardenMembership } from '@/lib/zen/garden-membership'
import { ZenWidgetData } from './ZenWidgetData'
import { DataSourcesSection } from './DataSourcesSection'
import { gardensLandingSelection, takeGardensLanding, useGardensLandingStore } from '../gardens-landing'

const MUTED = 'text-[11px] text-[var(--color-text-muted)]'
const H3 = 'text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider'
const SIDE_ROW = 'w-full text-left px-3 py-2 text-xs no-drag cursor-pointer'
const SIDE_ON = 'bg-[var(--color-accent)]/15'
const SIDE_OFF = 'hover:bg-white/[0.03]'

/** "Sees" (G26, Rosson Q1): a Garden sees everything the person sees. */
export const GARDEN_SEES_TEXT = 'Everything you can see'

/** The last rail entry: widget data and data sources no Garden uses
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

function GripIcon(): React.JSX.Element {
  return (
    <svg width="8" height="12" viewBox="0 0 8 12" aria-hidden="true" fill="currentColor">
      <circle cx="2" cy="2" r="1" />
      <circle cx="6" cy="2" r="1" />
      <circle cx="2" cy="6" r="1" />
      <circle cx="6" cy="6" r="1" />
      <circle cx="2" cy="10" r="1" />
      <circle cx="6" cy="10" r="1" />
    </svg>
  )
}

/** The sidebar row's second line (G25): "N editors" and/or "made by X". */
export function gardenSidebarDetail(facts: GardenListFacts | undefined): string {
  if (!facts) return ''
  const parts: string[] = []
  if (facts.editors !== null) parts.push(`${facts.editors} editor${facts.editors === 1 ? '' : 's'}`)
  if (facts.madeBy) parts.push(`made by ${facts.madeBy.name}`)
  return parts.join(' · ')
}

// ── Sidebar ─────────────────────────────────────────────────────────

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

  // Today's grip drag (ZenGardensSection): a 3px threshold, the accent drop
  // line, one `garden/reorder` with the final spot.
  const onGripMouseDown = (e: React.MouseEvent, from: number): void => {
    if (e.button !== 0) return
    e.preventDefault()
    const g = gardens[from]
    const startY = e.clientY
    let started = false
    let slot: number | null = null
    const onMoveEv = (ev: MouseEvent): void => {
      if (!started && Math.abs(ev.clientY - startY) > 3) {
        started = true
        setDragId(g.id)
      }
      if (!started || !listRef.current) return
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
      setDragId(null)
      setDropSlot(null)
      if (slot === null || slot === from || slot === from + 1) return
      move(g, (slot > from ? slot - 1 : slot) + 1)
    }
    document.addEventListener('mousemove', onMoveEv)
    document.addEventListener('mouseup', onUp)
  }

  return (
    <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0" data-gardens-sidebar="">
      <div className="px-3 pt-3 pb-2 border-b border-[var(--color-border)]">
        <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Gardens</h2>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">Your Zen pages on this computer.</p>
      </div>
      <div className="flex-1 overflow-y-auto min-h-0">
        <button
          type="button"
          data-gardens-side="styles"
          aria-label="Styles"
          aria-current={sel.kind === 'styles' ? 'page' : undefined}
          className={`${SIDE_ROW} border-b border-[var(--color-border)] ${sel.kind === 'styles' ? SIDE_ON : SIDE_OFF}`}
          onClick={() => onSelect({ kind: 'styles' })}
        >
          <span className="block text-[var(--color-text-primary)]">Styles</span>
          <span className="block text-[10px] text-[var(--color-text-muted)]">The look of every Garden</span>
        </button>
        <div ref={listRef} data-gardens-side-list="">
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
                  <div className="absolute left-0 right-0 -top-px h-[2px] bg-[var(--color-accent)] z-10" />
                )}
                {dropSlot === gardens.length && i === gardens.length - 1 && dragId !== null && (
                  <div className="absolute left-0 right-0 -bottom-px h-[2px] bg-[var(--color-accent)] z-10" />
                )}
                <button
                  type="button"
                  aria-label={`Reorder ${g.name}`}
                  aria-roledescription="drag handle"
                  title="Drag to reorder"
                  data-garden-side-grip={g.id}
                  onMouseDown={(e) => onGripMouseDown(e, i)}
                  onKeyDown={(e) => {
                    if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return
                    e.preventDefault()
                    move(g, i + 1 + (e.key === 'ArrowUp' ? -1 : 1))
                  }}
                  className="w-5 flex-shrink-0 flex items-center justify-center no-drag cursor-grab text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
                >
                  <GripIcon />
                </button>
                <button
                  type="button"
                  data-gardens-side="garden"
                  data-gardens-side-id={g.id}
                  aria-label={`Open ${g.name}`}
                  aria-current={on ? 'page' : undefined}
                  className={`flex-1 min-w-0 text-left pr-3 py-2 no-drag cursor-pointer ${on ? SIDE_ON : SIDE_OFF}`}
                  onClick={() => onSelect({ kind: 'garden', id: g.id })}
                >
                  <span className="flex items-center gap-1.5 text-xs text-[var(--color-text-primary)]">
                    <span className="truncate">{g.name}</span>
                    {locked(g.id) ? (
                      <span className="text-[var(--color-text-muted)]" data-garden-side-locked="">
                        <LockGlyph />
                      </span>
                    ) : null}
                  </span>
                  {detail ? (
                    <span className="block text-[10px] text-[var(--color-text-muted)] truncate" data-garden-side-detail="">
                      {detail}
                    </span>
                  ) : null}
                </button>
              </div>
            )
          })}
        </div>
        <div className="px-3 py-2">
          <NewGarden gardens={gardens} className={BTN} />
        </div>
        {deleted !== null && deleted.length > 0 ? (
          <div className="border-t border-[var(--color-border)]" data-gardens-deleted="">
            <button
              type="button"
              aria-expanded={deletedOpen}
              data-gardens-deleted-toggle=""
              className={`${SIDE_ROW} text-[var(--color-text-secondary)] ${SIDE_OFF}`}
              onClick={() => setDeletedOpen((o) => !o)}
            >
              {deletedOpen ? '▾' : '▸'} Deleted Gardens ({deleted.length})
            </button>
            {deletedOpen &&
              deleted.map((d) => {
                const on = sel.kind === 'deleted' && sel.id === d.id
                return (
                  <button
                    key={d.id}
                    type="button"
                    data-gardens-side="deleted"
                    data-gardens-side-id={d.id}
                    aria-label={`Open deleted ${d.name}`}
                    className={`${SIDE_ROW} pl-6 text-[var(--color-text-muted)] ${on ? SIDE_ON : SIDE_OFF}`}
                    onClick={() => onSelect({ kind: 'deleted', id: d.id })}
                  >
                    {d.name}
                  </button>
                )
              })}
          </div>
        ) : null}
        <button
          type="button"
          data-gardens-side="unplaced"
          aria-label={NOT_IN_A_GARDEN}
          aria-current={sel.kind === 'unplaced' ? 'page' : undefined}
          className={`${SIDE_ROW} border-t border-[var(--color-border)] ${sel.kind === 'unplaced' ? SIDE_ON : SIDE_OFF}`}
          onClick={() => onSelect({ kind: 'unplaced' })}
        >
          <span className="block text-[var(--color-text-primary)]">{NOT_IN_A_GARDEN}</span>
          <span className="block text-[10px] text-[var(--color-text-muted)]">Data and sources no Garden uses</span>
        </button>
      </div>
    </div>
  )
}

// ── A Garden's data (Rosson 2026-10-23) ─────────────────────────────

/** Local Data and External Data for one rail entry: a Garden (`garden`
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
        <InlineError text={error} testId="garden-membership-error" />
        <button type="button" className={`${BTN} mt-2`} onClick={onRetry}>
          Try again
        </button>
      </div>
    )
  }
  if (!membership || !onlySources) return <p className={MUTED}>Loading…</p>
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
    <div className="max-w-2xl space-y-6" data-gardens-pane="unplaced">
      <div>
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">{NOT_IN_A_GARDEN}</h2>
        <p className="text-xs text-[var(--color-text-muted)]">
          Widget data and data sources on no Garden&apos;s page. A widget or source a Garden uses is under that Garden.
        </p>
      </div>
      <GardenDataSections garden={null} {...props} />
    </div>
  )
}

// ── Styles (G25) ────────────────────────────────────────────────────

function StylesPane({ onHistory }: { onHistory(t: GardenHistoryTarget): void }): React.JSX.Element {
  const st = useZenSettingsStore()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
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

  const reset = async (): Promise<void> => {
    const yes = await useConfirmDialogStore.getState().confirm({
      title: 'Reset Zen settings?',
      message: 'zen.toml goes back to K2’s defaults. K2 keeps the current file in history first.',
      confirmLabel: 'Reset',
      destructive: true,
    })
    if (yes && (await attempt(resetZenSettingsFile, setBusy, setError))) loadRecent()
  }

  return (
    <div className="max-w-2xl space-y-6" data-gardens-pane="styles" data-settings-id="zen-gardens.advanced">
      <div>
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Styles</h2>
        <p className="text-xs text-[var(--color-text-muted)]">The look every Garden uses on this computer.</p>
      </div>
      <section data-settings-id="zen-gardens.theme">
        <h3 className={H3}>Theme</h3>
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <div className="text-xs text-[var(--color-text-primary)]">Global theme</div>
          <SettingDropdown
            ariaLabel="Global Zen theme"
            value={st.globalTheme ?? ''}
            options={themeOptions(st.themes)}
            placeholder={st.globalTheme ?? 'No theme'}
            disabled={busy || st.themes.length === 0}
            onChange={(v) => void attempt(() => setZenGlobalTheme(v), setBusy, setError)}
          />
        </div>
        <ul className="mt-1" data-gardens-theme-list="">
          {st.themes.map((t) => (
            <li key={t.name} className="flex items-baseline gap-2 py-1 text-xs text-[var(--color-text-primary)]">
              <span className="font-mono">{t.name}</span>
              <span className={MUTED}>
                {t.builtin ? 'built in' : 'yours'}
                {t.summary ? ` · ${t.summary}` : ''}
                {t.name === st.globalTheme ? ' · in use' : ''}
              </span>
            </li>
          ))}
        </ul>
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <div className="text-xs text-[var(--color-text-primary)]">
            Zen settings <span className={`${MUTED} font-mono`}>zen.toml</span>
          </div>
          <button type="button" className={BTN} disabled={busy} data-gardens-reset-zen="" onClick={() => void reset()}>
            Reset
          </button>
        </div>
        <InlineError text={error} testId="gardens-styles-error" />
      </section>
      {st.path ? (
        <section data-settings-id="zen-gardens.folder">
          <h3 className={H3}>Folder</h3>
          <div className="flex items-center justify-between gap-3 py-2 border-b border-[var(--color-border)]">
            <span className="text-[11px] font-mono text-[var(--color-text-primary)] select-all truncate" data-zen-gardens-path="">
              {st.path}
            </span>
            <button
              type="button"
              className={`${BTN} flex-shrink-0`}
              onClick={() => {
                setRevealError(null)
                revealItemInDir(st.path as string).catch((err: unknown) =>
                  setRevealError(err instanceof Error ? err.message : String(err)),
                )
              }}
            >
              {revealLabel()}
            </button>
          </div>
          <InlineError text={revealError} testId="gardens-reveal-error" />
        </section>
      ) : null}
      <section data-gardens-look-history="">
        <div className="flex items-center justify-between">
          <h3 className={H3}>Recent look changes</h3>
          <button
            type="button"
            className={BTN}
            onClick={() => onHistory({ kind: 'garden', garden: '*', name: 'Styles' })}
          >
            History…
          </button>
        </div>
        <InlineError text={recentError} testId="gardens-look-history-error" />
        {recent !== null && recent.length === 0 ? <p className={`${MUTED} mt-2`}>No changes yet.</p> : null}
        <ul className="mt-1">
          {(recent ?? []).map((it) => (
            <li key={it.id} className="py-1 text-xs text-[var(--color-text-primary)]" data-gardens-look-line={it.id}>
              {gardenHistoryLine(it)} <span className={MUTED}>· {gardenWhen(it.at)}</span>
            </li>
          ))}
        </ul>
      </section>
    </div>
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
  const menu = useAnchoredMenu<HTMLButtonElement>({ open, onClose: () => setOpen(false), gap: 4, width: 'min', minWidth: 280 })

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
      <button
        ref={menu.anchorRef}
        type="button"
        className={`${BTN} mt-2`}
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        data-garden-add-agent=""
        onClick={() => {
          setQuery('')
          setOpen((o) => !o)
        }}
      >
        + Add agent
      </button>
      {open &&
        menu.portal(
          <div
            ref={menu.menuRef}
            data-garden-add-agent-picker=""
            className="no-drag border border-[var(--color-border)] bg-[var(--color-bg-elevated)] shadow-lg p-2"
            style={menu.style}
          >
            {error ? (
              <p role="alert" className="text-[11px] text-[var(--color-status-error)] p-1">
                {error}
              </p>
            ) : agents === null ? (
              <p className={`${MUTED} p-1`}>Loading…</p>
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

function GardenMenu({ onDelete, name }: { onDelete(): void; name: string }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const menu = useAnchoredMenu<HTMLButtonElement>({ open, onClose: () => setOpen(false), gap: 4, width: 'min', minWidth: 140, align: 'end' })
  return (
    <>
      <button
        ref={menu.anchorRef}
        type="button"
        aria-label={`More for ${name}`}
        aria-haspopup="menu"
        aria-expanded={open}
        data-garden-pane-more=""
        className={BTN}
        onClick={() => setOpen((o) => !o)}
      >
        ⋯
      </button>
      {open &&
        menu.portal(
          <div
            ref={menu.menuRef}
            role="menu"
            className="no-drag flex flex-col border border-[var(--color-border)] bg-[var(--color-bg-elevated)] shadow-lg p-1"
            style={menu.style}
          >
            <button
              type="button"
              role="menuitem"
              data-garden-pane-delete=""
              className="text-left px-3 py-1.5 text-xs text-[var(--color-status-error)] hover:bg-white/[0.04] cursor-pointer"
              onClick={() => {
                setOpen(false)
                onDelete()
              }}
            >
              Delete
            </button>
          </div>,
        )}
    </>
  )
}

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
  const [draft, setDraft] = useState(garden.name)
  const seq = useRef(0)
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

  const run = async (fn: () => Promise<unknown>): Promise<boolean> => {
    setBusy(true)
    setError(null)
    try {
      await fn()
      return true
    } catch (err) {
      setError(gardenAccessErrorText(err))
      return false
    } finally {
      setBusy(false)
      load()
    }
  }

  const add = (a: LocalAgent): void => {
    void run(() => setGardenEditor(id, a.id, true)).then((ok) => {
      if (ok) useToastStore.getState().addToast(`${a.name} can edit ${garden.name}`, 'success')
    })
  }

  // G26: Remove with an 8 s Undo toast (ZenGardenSyncControls' toast).
  const remove = (agent: string, name: string): void => {
    void run(() => setGardenEditor(id, agent, false)).then((ok) => {
      if (!ok) return
      useToastStore.getState().addToast(`${name} can no longer edit ${garden.name}`, 'success', 8000, {
        label: 'Undo',
        onClick: () =>
          void setGardenEditor(id, agent, true)
            .catch((err: unknown) => setError(gardenAccessErrorText(err)))
            .finally(load),
      })
    })
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
      setRenaming(false)
      return
    }
    if (await attempt(() => renameZenSettingsGardenNow(id, clean), setBusy, setError)) setRenaming(false)
  }

  const askDelete = async (): Promise<void> => {
    const yes = await useConfirmDialogStore.getState().confirm({
      title: `Delete ${garden.name}?`,
      message: 'Its page moves to history in the Gardens folder, so a copy stays.',
      confirmLabel: 'Delete',
      destructive: true,
    })
    if (yes && (await attempt(() => deleteZenSettingsGarden(id), setBusy, setError))) onDeleted()
  }

  const listed = useMemo(() => new Set((access?.editors ?? []).map((e) => e.agent)), [access])
  return (
    <div className="max-w-2xl space-y-6" data-gardens-pane="garden" data-garden-pane={id}>
      <div>
        <div className="flex items-center gap-2">
          {renaming ? (
            <input
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
                  setRenaming(false)
                  setDraft(garden.name)
                }
              }}
              className={`${INPUT} flex-1 text-sm`}
            />
          ) : (
            <h2 className="flex-1 min-w-0 truncate text-lg font-semibold text-[var(--color-text-primary)]" data-garden-pane-title="">
              {garden.name}
            </h2>
          )}
          <button
            type="button"
            className={BTN}
            data-garden-pane-history=""
            onClick={() => onHistory({ kind: 'garden', garden: id, name: garden.name })}
          >
            History…
          </button>
          {renaming ? (
            <button type="button" className={BTN} disabled={busy} onClick={() => void saveRename()}>
              Save
            </button>
          ) : (
            <button type="button" className={BTN} data-garden-pane-rename="" onClick={() => setRenaming(true)}>
              Rename
            </button>
          )}
          <GardenMenu name={garden.name} onDelete={() => void askDelete()} />
        </div>
        {access?.made ? (
          <p className={`${MUTED} mt-1`} data-garden-pane-made="">
            Made by {access.made.name}
            {access.made.at !== null ? ` · ${gardenDay(access.made.at)}` : ''}
          </p>
        ) : null}
        <InlineError text={error} testId="garden-pane-error" />
      </div>

      {loadError ? (
        <div>
          <InlineError text={loadError} testId="garden-pane-load-error" />
          <button type="button" className={`${BTN} mt-2`} onClick={load}>
            Try again
          </button>
        </div>
      ) : null}

      <section data-garden-editors="">
        <h3 className={H3}>Agents who can edit</h3>
        {access === null && !loadError ? <p className={`${MUTED} mt-2`}>Loading…</p> : null}
        {access !== null && access.editors.length === 0 ? (
          <p className={`${MUTED} mt-2`} data-garden-editors-empty="">
            No agents yet. An agent that asks gets a card on its Thread with you.
          </p>
        ) : null}
        <ul className="mt-1">
          {(access?.editors ?? []).map((e) => (
            <li
              key={e.agent}
              data-garden-editor={e.agent}
              className="flex items-center gap-2 py-1.5 border-b border-[var(--color-border)]"
            >
              <span
                aria-hidden="true"
                className="w-2 h-2 rounded-full flex-shrink-0"
                style={{ background: e.color ?? 'var(--color-text-muted)' }}
              />
              <span className="text-xs text-[var(--color-text-primary)]">{e.name}</span>
              <span className={`${MUTED} flex-1 min-w-0 truncate`} data-garden-editor-added="">
                {gardenEditorAddedWords(e)}
              </span>
              <button
                type="button"
                className={BTN}
                disabled={busy}
                aria-label={`Remove ${e.name}`}
                data-garden-editor-remove={e.agent}
                onClick={() => remove(e.agent, e.name)}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
        {access !== null ? <AddAgent listed={listed} onPick={add} disabled={busy} /> : null}
      </section>

      <section className="flex items-baseline gap-4" data-garden-sees="">
        <h3 className={H3}>Sees</h3>
        <span className="text-xs text-[var(--color-text-primary)]">{GARDEN_SEES_TEXT}</span>
      </section>

      <section data-garden-pane-controls="">
        <ZenGardenSyncControls garden={garden} variant="pane" />
      </section>

      <section data-garden-widgets="">
        <h3 className={H3}>Widgets</h3>
        {widgets !== null && widgets.length === 0 ? <p className={`${MUTED} mt-2`}>No widgets made for this Garden yet.</p> : null}
        <ul className="mt-1">
          {(widgets ?? []).map((w) => (
            <li
              key={w.widget}
              data-garden-widget={w.widget}
              className="flex items-center gap-2 py-1.5 border-b border-[var(--color-border)]"
            >
              <span className="text-xs font-mono text-[var(--color-text-primary)]">{w.widget}</span>
              <span className={`${MUTED} flex-1 min-w-0 truncate`} data-garden-widget-signer="">
                {gardenWidgetSignerWords(w)}
              </span>
              <button
                type="button"
                className={BTN}
                data-garden-widget-history={w.widget}
                onClick={() => onHistory({ kind: 'widget', widget: w.widget, garden: id })}
              >
                History…
              </button>
            </li>
          ))}
        </ul>
      </section>

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
  return (
    <div className="max-w-2xl space-y-4" data-gardens-pane="deleted" data-garden-deleted-pane={garden.id}>
      <div>
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">{garden.name}</h2>
        <p className={`${MUTED} mt-1`}>
          Deleted
          {garden.deletedBy ? ` by ${gardenActorWords(garden.deletedBy).replace(/^You$/, 'you')}` : ''}
          {garden.deletedAt !== null ? ` · ${gardenDay(garden.deletedAt)}` : ''}. K2 keeps a copy in history.
        </p>
      </div>
      <div className="flex gap-2">
        <button
          type="button"
          className={BTN}
          disabled={busy}
          data-garden-restore=""
          onClick={() => {
            setBusy(true)
            setError(null)
            restoreDeletedGarden(garden.id)
              .then(onRestored)
              .catch((err: unknown) => setError(gardenAccessErrorText(err)))
              .finally(() => setBusy(false))
          }}
        >
          Restore
        </button>
        <button
          type="button"
          className={BTN}
          data-garden-deleted-history=""
          onClick={() => onHistory({ kind: 'garden', garden: garden.id, name: garden.name, readOnly: true })}
        >
          History…
        </button>
      </div>
      <InlineError text={error} testId="garden-restore-error" />
    </div>
  )
}

// ── The page ────────────────────────────────────────────────────────

export function ZenGardensAccessPage(): React.JSX.Element {
  const st = useZenSettingsStore()
  // The sidebar's lock glyphs read the sync store; keep it live here too
  // (the pane's checkboxes aren't mounted while Styles is open).
  useZenSyncLive()
  const sync = useZenSyncStore((s) => s.list)
  const [sel, setSel] = useState<GardensSelection>({ kind: 'styles' })
  const [facts, setFacts] = useState<Map<string, GardenListFacts>>(new Map())
  const [deleted, setDeleted] = useState<DeletedGarden[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [history, setHistory] = useState<GardenHistoryTarget | null>(null)

  // The sidebar's facts and Deleted Gardens: re-read with the Garden list
  // (it re-reads on every local `zen_changed`) and on window focus.
  const loadSide = useCallback(() => {
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
      <Sidebar
        gardens={st.gardens}
        facts={facts}
        locked={locked}
        deleted={deleted}
        sel={sel}
        onSelect={setSel}
      />
      <div className="flex-1 overflow-y-auto p-6 min-h-0">
        {error ? (
          <p role="alert" className="text-[11px] text-[var(--color-status-error)] max-w-2xl mb-3" data-gardens-error="">
            {error}
          </p>
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
        {sel.kind === 'unplaced' ? (
          <UnplacedPane membership={membership} error={membershipError} onRetry={loadMembership} />
        ) : null}
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
      {history ? (
        <ZenGardenHistoryModal target={history} vars={ZEN_TOKENS_IN_SETTINGS} onClose={() => setHistory(null)} />
      ) : null}
    </div>
  )
}
