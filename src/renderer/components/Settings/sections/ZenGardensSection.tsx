// Settings → Gardens → Gardens (prd-zen-gardens-v1 item 17;
// prd-garden-lock-and-sync-v1 §3, Rosson 2026-10-13). The first page of the
// Gardens group (0.45.7, Rosson: Gardens get their own section, like K2
// Server and Logs); Widget data is the group's second page
// (`ZenWidgetDataSection`). This computer's Zen Gardens, one row each: the
// grip to drag it into place, the name as an in-place editor with Save,
// Delete (asks in K2's confirm dialog) (Rosson 2026-10-13), and two checkboxes,
// "Lock garden" and "Keep in sync with future template updates"
// (`ZenGardenSyncControls`). Above the list: + New Garden (the same modal as
// Zen's) and a search by name. Advanced (always shown, the right half behind
// a full-height divider, laid out like Settings → LLMs) holds the global
// theme and the folder. Thin client over
// `lib/zen/zen-settings.ts`: every request goes to the LOCAL daemon. Re-reads
// on every local `zen_changed`, like the Garden switcher, so Settings and an
// open Zen window stay in step.
//
// Settings only shows this where the Zen toggle shows
// (`useZenGardensSettingsShown`); the web client never mounts it.

import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { revealItemInDir } from '@tauri-apps/plugin-opener'
import type { SettingEntry } from '../searchManifest'
import { SettingDropdown } from '../controls/SettingControls'
import { ZenGardenSyncControls } from './ZenGardenSyncControls'
import { watchLocalZenChanged } from '@/lib/zen/zen-api'
import { currentDesktopOs } from '@/lib/zen/zen-platform'
import {
  checkZenGardenName,
  createZenSettingsGarden,
  deleteZenSettingsGarden,
  loadZenSettings,
  reorderZenSettingsGarden,
  renameZenSettingsGardenNow,
  setZenGlobalTheme,
  setupZenGardens,
  useZenSettingsStore,
  zenSettingsErrorText,
  type ZenNewGardenTemplate,
  type ZenSettingsGarden,
  type ZenSettingsTheme,
} from '@/lib/zen/zen-settings'
import { ZenNewGardenModal } from '@/components/Zen/widgets/ZenNewGardenModal'
import { useToastStore } from '@/stores/toast'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

export const ZEN_GARDENS_MANIFEST: SettingEntry[] = [
  {
    id: 'zen-gardens.list',
    section: 'zen-gardens',
    label: 'Gardens',
    description: 'Lock, sync, rename, reorder or delete your Zen Gardens',
    keywords: ['zen', 'garden', 'gardens', 'lock', 'unlock', 'sync', 'template', 'rename', 'reorder', 'delete', 'new garden'],
  },
  {
    // The Advanced section (always shown): a search lands on it, the theme
    // and folder rows open in front of the person.
    id: 'zen-gardens.advanced',
    section: 'zen-gardens',
    label: 'Zen Theme and Gardens Folder',
    description: 'Advanced: the theme every Garden uses, and where Gardens live (~/.k2/zen)',
    keywords: ['zen', 'theme', 'garden', 'appearance', 'folder', 'path', 'finder', 'advanced'],
  },
]

const BTN =
  'px-2 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default'
// The two columns are Settings → LLMs' (AgentsSection), class for class
// (Rosson 2026-10-13): a full-height row, two equal columns that each
// scroll on their own and carry their own padding, the right one with the
// divider (`border-l`). The padding is inside the columns, so the line
// touches the top and bottom of the pane, in the middle. Settings gives
// both pages `overflow-hidden p-0`.
const COLUMNS = 'flex h-full min-h-0'
const LIST_COLUMN = 'flex-1 min-w-0 overflow-y-auto p-6 pr-3 [scrollbar-gutter:stable]'
const ADVANCED_COLUMN =
  'flex-1 min-w-0 overflow-y-auto border-l border-[var(--color-border)] p-6 pl-6 pr-3 [scrollbar-gutter:stable]'

/** A Garden's name: reads as plain text, edits in place (border on hover/focus). */
const NAME_INPUT =
  'w-full px-1 py-0.5 -ml-1 text-xs bg-transparent border border-transparent text-[var(--color-text-primary)] outline-none hover:border-[var(--color-border)] focus:border-[var(--color-accent)] focus:bg-[var(--color-bg)] no-drag cursor-text truncate'
const DANGER_BTN =
  'px-2 py-1 text-xs border border-[var(--color-status-error)] text-[var(--color-status-error)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default'
const INPUT =
  'px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

function InlineError({ text, testId }: { text: string | null; testId?: string }): React.JSX.Element | null {
  if (!text) return null
  return (
    <p role="alert" data-testid={testId} className="text-[11px] text-[var(--color-status-error)] mt-1">
      {text}
    </p>
  )
}

function revealLabel(): string {
  const os = currentDesktopOs()
  if (os === 'mac') return 'Reveal in Finder'
  if (os === 'windows') return 'Show in Explorer'
  return 'Show in Files'
}

/** Run `fn`, keeping `busy` on meanwhile and putting a failure in `setError`. */
async function attempt(
  fn: () => Promise<void>,
  setBusy: (b: boolean) => void,
  setError: (e: string | null) => void,
): Promise<boolean> {
  setBusy(true)
  setError(null)
  try {
    await fn()
    return true
  } catch (err) {
    setError(zenSettingsErrorText(err))
    return false
  } finally {
    setBusy(false)
  }
}

function themeOptions(themes: readonly ZenSettingsTheme[]): { value: string; label: string }[] {
  return themes.map((t) => ({ value: t.name, label: t.name }))
}

/** Where the drop line paints on a row: above it, below it, or nowhere. */
type DropEdge = 'top' | 'bottom' | null

/** Six-dot grip, drawn in the text colour. */
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

function GardenRow({
  garden,
  gardens,
  dragging,
  picked,
  dropEdge,
  reorderable,
  onHandleMouseDown,
  onHandleKeyDown,
  onHandleBlur,
}: {
  garden: ZenSettingsGarden
  gardens: readonly ZenSettingsGarden[]
  /** This row is the one being dragged (mouse) or carried (keyboard). */
  dragging: boolean
  /** Carried with the keyboard (Space picked it up). */
  picked: boolean
  dropEdge: DropEdge
  /** False while a search filters the list: the grip is dimmed and inert. */
  reorderable: boolean
  onHandleMouseDown(e: React.MouseEvent): void
  onHandleKeyDown(e: React.KeyboardEvent): void
  onHandleBlur(): void
}): React.JSX.Element {
  const [draft, setDraft] = useState(garden.name)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  // A rename from elsewhere (Zen, the CLI) shows here unless the person is
  // mid-edit (the field still held the old name).
  const shownName = useRef(garden.name)
  useEffect(() => {
    setDraft((d) => (d === shownName.current ? garden.name : d))
    shownName.current = garden.name
  }, [garden.name])

  const trimmed = draft.trim()
  const canSave = !busy && trimmed !== '' && trimmed !== garden.name

  const revert = (): void => {
    setDraft(garden.name)
    setError(null)
  }

  const saveRename = async (): Promise<void> => {
    if (!canSave) return
    let clean: string
    try {
      clean = checkZenGardenName(draft, gardens, garden.id)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      return
    }
    const old = garden.name
    setError(null)
    setBusy(true)
    setDraft(clean)
    try {
      await renameZenSettingsGardenNow(garden.id, clean)
    } catch (err) {
      setDraft(old)
      useToastStore.getState().addToast(`Couldn’t rename “${old}”: ${zenSettingsErrorText(err, clean)}`, 'error')
    } finally {
      setBusy(false)
    }
  }

  const askDelete = async (): Promise<void> => {
    setError(null)
    const yes = await useConfirmDialogStore.getState().confirm({
      title: `Delete ${garden.name}?`,
      message: 'Its page moves to history in the Gardens folder, so a copy stays.',
      confirmLabel: 'Delete',
      destructive: true,
    })
    if (yes) await attempt(() => deleteZenSettingsGarden(garden.id), setBusy, setError)
  }

  return (
    <li
      data-zen-garden-row={garden.id}
      className={`relative py-2 border-b border-[var(--color-border)] ${dragging ? 'opacity-30' : ''}`}
    >
      {dropEdge === 'top' && (
        <div data-zen-garden-drop-line="top" className="absolute left-0 right-0 -top-px h-[2px] bg-[var(--color-accent)] z-10" />
      )}
      {dropEdge === 'bottom' && (
        <div data-zen-garden-drop-line="bottom" className="absolute left-0 right-0 -bottom-px h-[2px] bg-[var(--color-accent)] z-10" />
      )}
      <div className="flex items-center gap-2">
        <button
          type="button"
          data-zen-garden-handle={garden.id}
          aria-label={`Reorder ${garden.name}`}
          aria-roledescription="drag handle"
          aria-pressed={picked}
          aria-describedby="zen-gardens-reorder-help"
          aria-disabled={!reorderable}
          title={reorderable ? 'Drag to reorder' : 'Clear search to reorder'}
          onMouseDown={onHandleMouseDown}
          onKeyDown={onHandleKeyDown}
          onBlur={onHandleBlur}
          className={`w-4 h-6 flex-shrink-0 flex items-center justify-center no-drag outline-none focus-visible:outline focus-visible:outline-1 focus-visible:outline-[var(--color-accent)] ${
            reorderable
              ? 'cursor-grab active:cursor-grabbing hover:text-[var(--color-text-primary)] focus:text-[var(--color-text-primary)]'
              : 'cursor-default opacity-30'
          } ${picked ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-muted)]'}`}
        >
          <GripIcon />
        </button>
        {/* The name is the editor: plain text until hovered or focused. */}
        <div className="flex-1 min-w-0">
          <input
            ref={inputRef}
            data-zen-garden-name={garden.id}
            aria-label={`Name of ${garden.name}`}
            title="Rename"
            value={draft}
            disabled={busy}
            maxLength={80}
            spellCheck={false}
            onChange={(e) => {
              setDraft(e.target.value)
              setError(null)
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault()
                void saveRename()
              } else if (e.key === 'Escape') {
                e.preventDefault()
                e.stopPropagation()
                revert()
                inputRef.current?.blur()
              }
            }}
            className={NAME_INPUT}
          />
        </div>
        <button
          type="button"
          aria-label={`Save name of ${garden.name}`}
          disabled={!canSave}
          onClick={() => void saveRename()}
          className={BTN}
        >
          Save
        </button>
        <button
          type="button"
          aria-label={`Delete ${garden.name}`}
          title="Delete"
          disabled={busy}
          onClick={() => void askDelete()}
          className={DANGER_BTN}
        >
          Delete
        </button>
      </div>

      <ZenGardenSyncControls garden={garden} />
      <InlineError text={error} testId={`zen-garden-error-${garden.id}`} />
    </li>
  )
}

/** Drop slot `drop` (0..n, before row `drop`) for row `from` as a 1-based
 *  position, or null when it would land where it already is. */
function slotToPosition(from: number, drop: number): number | null {
  if (drop === from || drop === from + 1) return null
  return (drop > from ? drop - 1 : drop) + 1
}

/**
 * The Gardens, in order, reordered by dragging a row's grip (the Agents
 * presets' mouse drag and accent drop line) or from the keyboard: on the
 * grip, ArrowUp/ArrowDown moves the Garden one place; Space/Enter picks it
 * up, the arrows choose a spot, Space/Enter drops it, Escape cancels. One
 * `zen/garden/reorder` per move with the final position; the list paints
 * at once and goes back (with a toast) if the daemon refuses.
 *
 * `query` filters by name (case-insensitive substring). While it filters,
 * reordering is off (a partial list would make a confusing order): the
 * grips dim and say "Clear search to reorder". Rename, Delete and the
 * checkboxes still work on the rows shown.
 */
function GardenList({
  gardens: all,
  query,
}: {
  gardens: readonly ZenSettingsGarden[]
  query: string
}): React.JSX.Element {
  const q = query.trim().toLocaleLowerCase()
  const filtering = q !== ''
  const gardens = filtering ? all.filter((g) => g.name.toLocaleLowerCase().includes(q)) : all
  const listRef = useRef<HTMLOListElement>(null)
  const [dragId, setDragId] = useState<string | null>(null)
  const [dropSlot, setDropSlot] = useState<number | null>(null)
  const [pickedId, setPickedId] = useState<string | null>(null)
  const [announce, setAnnounce] = useState('')
  const pending = useRef(false)
  const refocus = useRef<string | null>(null)
  const count = gardens.length

  // A moved row's node is re-inserted, which can drop focus: put it back.
  useLayoutEffect(() => {
    const id = refocus.current
    if (!id || !listRef.current) return
    const handle = [...listRef.current.querySelectorAll<HTMLElement>('[data-zen-garden-handle]')].find(
      (el) => el.getAttribute('data-zen-garden-handle') === id,
    )
    if (handle && document.activeElement !== handle) handle.focus()
  })

  const move = useCallback(
    (garden: ZenSettingsGarden, to: number): void => {
      if (pending.current) return
      pending.current = true
      setAnnounce(`${garden.name} moved to position ${to} of ${count}.`)
      reorderZenSettingsGarden(garden.id, to)
        .catch((err: unknown) => {
          const why = zenSettingsErrorText(err)
          setAnnounce(`Couldn’t move ${garden.name}. ${why}`)
          useToastStore.getState().addToast(`Couldn’t move “${garden.name}”: ${why}`, 'error')
        })
        .finally(() => {
          pending.current = false
        })
    },
    [count],
  )

  const onHandleMouseDown = (e: React.MouseEvent, from: number): void => {
    if (e.button !== 0 || filtering) return
    e.preventDefault()
    const garden = gardens[from]
    const startY = e.clientY
    let started = false
    let slot: number | null = null
    const onMoveEv = (ev: MouseEvent): void => {
      if (!started && Math.abs(ev.clientY - startY) > 3) {
        started = true
        setPickedId(null)
        setDragId(garden.id)
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }
      if (!started || !listRef.current) return
      const rows = listRef.current.querySelectorAll('[data-zen-garden-row]')
      let idx = 0
      for (let i = 0; i < rows.length; i++) {
        const rect = rows[i].getBoundingClientRect()
        if (ev.clientY > rect.top + rect.height / 2) idx = i + 1
      }
      slot = idx
      setDropSlot(idx)
    }
    const onUp = (): void => {
      document.removeEventListener('mousemove', onMoveEv)
      document.removeEventListener('mouseup', onUp)
      if (!started) return
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
      setDragId(null)
      setDropSlot(null)
      const to = slot === null ? null : slotToPosition(from, slot)
      if (to !== null) move(garden, to)
    }
    document.addEventListener('mousemove', onMoveEv)
    document.addEventListener('mouseup', onUp)
  }

  const stopCarrying = (): void => {
    setPickedId(null)
    setDropSlot(null)
  }

  const onHandleKeyDown = (e: React.KeyboardEvent, from: number): void => {
    if (filtering) return
    const garden = gardens[from]
    const carrying = pickedId === garden.id
    const key = e.key
    if (key === ' ' || key === 'Enter') {
      e.preventDefault()
      if (!carrying) {
        setPickedId(garden.id)
        setDropSlot(from)
        setAnnounce(
          `Picked up ${garden.name}, position ${from + 1} of ${count}. Arrow keys move it, Space drops it, Escape cancels.`,
        )
        return
      }
      const to = slotToPosition(from, dropSlot ?? from)
      stopCarrying()
      refocus.current = garden.id
      if (to === null) setAnnounce(`${garden.name} dropped at position ${from + 1} of ${count}.`)
      else move(garden, to)
      return
    }
    if (key === 'Escape' && carrying) {
      e.preventDefault()
      e.stopPropagation()
      stopCarrying()
      setAnnounce(`Move cancelled. ${garden.name} stays at position ${from + 1} of ${count}.`)
      return
    }
    if (key !== 'ArrowUp' && key !== 'ArrowDown') return
    e.preventDefault()
    const up = key === 'ArrowUp'
    if (carrying) {
      // The drop line walks one spot; the slot just below the row is the
      // same place as the one above it, so it is skipped.
      const cur = dropSlot ?? from
      let next = up ? cur - 1 : cur + 1
      if (next === from + 1) next = up ? from : from + 2
      if (next < 0 || next > count) return
      setDropSlot(next)
      setAnnounce(`${garden.name}, position ${slotToPosition(from, next) ?? from + 1} of ${count}.`)
      return
    }
    const to = from + 1 + (up ? -1 : 1)
    if (to < 1 || to > count) return
    refocus.current = garden.id
    move(garden, to)
  }

  const active = dragId ?? pickedId
  const from = active ? gardens.findIndex((g) => g.id === active) : -1
  const showLine = from >= 0 && dropSlot !== null && slotToPosition(from, dropSlot) !== null

  return (
    <>
      <p id="zen-gardens-reorder-help" className="sr-only">
        Drag to reorder. From the keyboard, the arrow keys move a Garden; or Space picks it up, the arrows choose a
        spot, Space drops it and Escape cancels.
      </p>
      <ol ref={listRef} className="border-t border-[var(--color-border)]" data-zen-gardens-list="">
        {gardens.map((g, i) => (
          <GardenRow
            key={g.id}
            garden={g}
            gardens={all}
            reorderable={!filtering}
            dragging={g.id === active}
            picked={g.id === pickedId}
            dropEdge={!showLine ? null : dropSlot === i ? 'top' : dropSlot === count && i === count - 1 ? 'bottom' : null}
            onHandleMouseDown={(e) => onHandleMouseDown(e, i)}
            onHandleKeyDown={(e) => onHandleKeyDown(e, i)}
            onHandleBlur={() => {
              if (pickedId === g.id) stopCarrying()
            }}
          />
        ))}
      </ol>
      {filtering && gardens.length === 0 && (
        <p data-zen-gardens-no-match="" className="py-3 text-xs text-[var(--color-text-muted)]">
          No Gardens match “{query.trim()}”
        </p>
      )}
      <div aria-live="polite" role="status" className="sr-only" data-zen-gardens-announce="">
        {announce}
      </div>
    </>
  )
}

/** Search the Gardens by name (Rosson 2026-10-13): the Settings search-field
 *  look, a × to clear, Escape clears too. Not remembered. */
function GardenSearch({ value, onChange }: { value: string; onChange(v: string): void }): React.JSX.Element {
  return (
    <div className="relative mb-3" data-zen-gardens-search="">
      <input
        type="search"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Escape' && value !== '') {
            e.preventDefault()
            e.stopPropagation()
            onChange('')
          }
        }}
        placeholder="Search Gardens"
        aria-label="Search Gardens"
        className={`${INPUT} w-full pr-7 [&::-webkit-search-cancel-button]:hidden`}
      />
      {value !== '' && (
        <button
          type="button"
          aria-label="Clear search"
          title="Clear search"
          onClick={() => onChange('')}
          className="absolute right-1 top-1/2 -translate-y-1/2 w-5 h-5 flex items-center justify-center text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
        >
          ×
        </button>
      )}
    </div>
  )
}

/** Zen's own tokens mapped onto Settings' look, so the shared New Garden
 *  modal and cards (drawn with `--zen-*`) read as part of this page. */
const ZEN_TOKENS_IN_SETTINGS = {
  '--zen-text': 'var(--color-text-primary)',
  '--zen-text-muted': 'var(--color-text-muted)',
  '--zen-surface': 'var(--color-bg)',
  '--zen-surface-raised': 'var(--color-bg-elevated)',
  '--zen-border': 'var(--color-border)',
  '--zen-accent': 'var(--color-accent)',
  '--zen-danger': 'var(--color-status-error)',
  '--zen-radius': '6px',
  color: 'var(--color-text-primary)',
} as React.CSSProperties

/**
 * Settings' New Garden: the same modal as Zen's (Rosson 2026-10-08): a
 * name and the catalog's cards, the Diary first. Creates through the local
 * daemon; there is nothing to allow.
 */
function NewGarden({ gardens }: { gardens: readonly ZenSettingsGarden[] }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button type="button" data-zen-settings-new-garden="" onClick={() => setOpen(true)} className={`${BTN} mb-3`}>
        + New Garden
      </button>
      {open && (
        <ZenNewGardenModal
          taken={gardens.map((g) => g.name)}
          highlight={null}
          vars={ZEN_TOKENS_IN_SETTINGS}
          onClose={() => setOpen(false)}
          onCreate={async (pick) => {
            const clean = checkZenGardenName(pick.name, gardens)
            await createZenSettingsGarden(clean, pick.entry.short as ZenNewGardenTemplate)
          }}
        />
      )}
    </>
  )
}

/** Advanced, always shown (Rosson 2026-10-13): the global theme and the folder. */
function Advanced({
  setUp,
  path,
  themes,
  globalTheme,
}: {
  setUp: boolean
  path: string | null
  themes: readonly ZenSettingsTheme[]
  globalTheme: string | null
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [revealError, setRevealError] = useState<string | null>(null)
  return (
    // The right column, behind the full-height divider (LLMs' right column).
    <section
      aria-labelledby="zen-gardens-advanced-title"
      className={ADVANCED_COLUMN}
      data-zen-gardens-advanced=""
      data-settings-id="zen-gardens.advanced"
    >
      <h3
        id="zen-gardens-advanced-title"
        className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider"
      >
        Advanced
      </h3>
        <div className="mt-2">
          {setUp && (
            <div data-settings-id="zen-gardens.theme">
              <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
                <div className="text-xs text-[var(--color-text-primary)]">Global theme</div>
                <SettingDropdown
                  ariaLabel="Global Zen theme"
                  value={globalTheme ?? ''}
                  options={themeOptions(themes)}
                  placeholder={globalTheme ?? 'No theme'}
                  disabled={busy || themes.length === 0}
                  onChange={(v) => void attempt(() => setZenGlobalTheme(v), setBusy, setError)}
                />
              </div>
              <InlineError text={error} testId="zen-gardens-theme-error" />
            </div>
          )}
          {path ? (
            <div data-settings-id="zen-gardens.folder">
              <div className="flex items-center justify-between gap-3 py-2 border-b border-[var(--color-border)]">
                <span className="text-[11px] font-mono text-[var(--color-text-primary)] select-all truncate" data-zen-gardens-path="">
                  {path}
                </span>
                {setUp ? (
                  <button
                    type="button"
                    onClick={() => {
                      setRevealError(null)
                      revealItemInDir(path).catch((err: unknown) =>
                        setRevealError(err instanceof Error ? err.message : String(err)),
                      )
                    }}
                    className={`${BTN} flex-shrink-0`}
                  >
                    {revealLabel()}
                  </button>
                ) : null}
              </div>
              <InlineError text={revealError} testId="zen-gardens-reveal-error" />
            </div>
          ) : null}
        </div>
    </section>
  )
}

export function ZenGardensSection(): React.JSX.Element {
  const st = useZenSettingsStore()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState('')

  useEffect(() => {
    void loadZenSettings()
    return watchLocalZenChanged(() => void loadZenSettings())
  }, [])

  // Before the first answer there is nothing to show but the error, if any.
  const hasData = st.path !== null || st.setUp || st.gardens.length > 0

  return (
    // Settings → LLMs' layout (AgentsSection): left = the title, New Garden,
    // search and the list; right = Advanced, always shown, behind the
    // full-height divider in the middle.
    <div className={COLUMNS} data-zen-gardens-section="" data-zen-gardens-columns="">
        <div className={LIST_COLUMN} data-zen-gardens-list-column="">
          <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Gardens</h2>
          <p className="text-xs text-[var(--color-text-muted)] mb-6">Your Zen pages on this computer.</p>
          {st.failure ? (
            <div className="mb-4">
              <InlineError text={zenSettingsErrorText(new Error(st.failure.message))} testId="zen-gardens-load-error" />
              <button type="button" onClick={() => void loadZenSettings()} className={`${BTN} mt-2`}>
                Try again
              </button>
            </div>
          ) : null}

          {!hasData ? (
            st.failure ? null : <p className="text-xs text-[var(--color-text-muted)]">Loading…</p>
          ) : !st.setUp ? (
            <div data-zen-gardens-setup="">
              <p className="text-xs text-[var(--color-text-secondary)] mb-3">Gardens aren’t set up on this computer yet.</p>
              <button
                type="button"
                disabled={busy}
                onClick={() => void attempt(setupZenGardens, setBusy, setError)}
                className={BTN}
              >
                Set up Gardens
              </button>
              <InlineError text={error} testId="zen-gardens-setup-error" />
            </div>
          ) : (
            <section data-settings-id="zen-gardens.list">
              <NewGarden gardens={st.gardens} />
              <GardenSearch value={query} onChange={setQuery} />
              <GardenList gardens={st.gardens} query={query} />
            </section>
          )}
        </div>

        {hasData && (st.setUp || st.path) ? (
          <Advanced setUp={st.setUp} path={st.path} themes={st.themes} globalTheme={st.globalTheme} />
        ) : null}
    </div>
  )
}
