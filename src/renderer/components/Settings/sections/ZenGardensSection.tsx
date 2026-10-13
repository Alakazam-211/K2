// Settings → Gardens (prd-zen-gardens-v1 item 17; prd-garden-lock-and-sync-v1
// §3, Rosson 2026-10-13). This computer's Zen Gardens, one row each: the
// grip to drag it into place (Rosson 2026-10-13), the name, a ⋯ menu
// (Rename, Delete) and two checkboxes,
// "Lock garden" and "Keep in sync with future template updates"
// (`ZenGardenSyncControls`). + New Garden opens the same modal as Zen's. A
// collapsed Advanced holds the global theme and the folder. Thin client over
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
  renameZenSettingsGarden,
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

export const ZEN_GARDENS_MANIFEST: SettingEntry[] = [
  {
    id: 'zen-gardens.list',
    section: 'zen-gardens',
    label: 'Gardens',
    description: 'Lock, sync, rename, reorder or delete your Zen Gardens',
    keywords: ['zen', 'garden', 'gardens', 'lock', 'unlock', 'sync', 'template', 'rename', 'reorder', 'delete', 'new garden'],
  },
  {
    // The collapsed Advanced section: a search lands on it (its inner rows
    // aren't in the page until it opens).
    id: 'zen-gardens.advanced',
    section: 'zen-gardens',
    label: 'Zen Theme and Gardens Folder',
    description: 'Advanced: the theme every Garden uses, and where Gardens live (~/.k2/zen)',
    keywords: ['zen', 'theme', 'garden', 'appearance', 'folder', 'path', 'finder', 'advanced'],
  },
]

const BTN =
  'px-2 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default'
const ICON_BTN =
  'w-6 h-6 flex items-center justify-center text-xs border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-30 disabled:cursor-default'
const MENU_ITEM =
  'block w-full text-left px-3 py-1 text-xs text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default'
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

/** A row's ⋯ menu: Rename, Delete. (Reorder is the row's drag handle.) */
function RowMenu({
  name,
  busy,
  onRename,
  onDelete,
}: {
  name: string
  busy: boolean
  onRename(): void
  onDelete(): void
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const box = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const away = (e: MouseEvent): void => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false)
    }
    const esc = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', away)
    document.addEventListener('keydown', esc)
    return () => {
      document.removeEventListener('mousedown', away)
      document.removeEventListener('keydown', esc)
    }
  }, [open])
  const pick = (fn: () => void) => (): void => {
    setOpen(false)
    fn()
  }
  return (
    <div ref={box} className="relative flex-shrink-0">
      <button
        type="button"
        aria-label={`More for ${name}`}
        aria-haspopup="menu"
        aria-expanded={open}
        title="More"
        disabled={busy}
        onClick={() => setOpen((o) => !o)}
        className={ICON_BTN}
      >
        ⋯
      </button>
      {open && (
        <div
          role="menu"
          aria-label={`${name} menu`}
          className="absolute right-0 top-7 z-[400] min-w-[140px] py-1 border border-[var(--color-border)] bg-[var(--color-bg)] shadow"
        >
          <button type="button" role="menuitem" className={MENU_ITEM} onClick={pick(onRename)}>
            Rename
          </button>
          <button type="button" role="menuitem" className={MENU_ITEM} onClick={pick(onDelete)}>
            Delete
          </button>
        </div>
      )}
    </div>
  )
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
  onHandleMouseDown(e: React.MouseEvent): void
  onHandleKeyDown(e: React.KeyboardEvent): void
  onHandleBlur(): void
}): React.JSX.Element {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(garden.name)
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (editing) inputRef.current?.select()
  }, [editing])

  const startRename = (): void => {
    setDraft(garden.name)
    setError(null)
    setConfirmDelete(false)
    setEditing(true)
  }

  const saveRename = async (): Promise<void> => {
    let clean: string
    try {
      clean = checkZenGardenName(draft, gardens, garden.id)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      return
    }
    if (clean === garden.name) {
      setEditing(false)
      setError(null)
      return
    }
    const ok = await attempt(() => renameZenSettingsGarden(garden.id, clean), setBusy, setError)
    if (ok) setEditing(false)
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
          title="Drag to reorder"
          onMouseDown={onHandleMouseDown}
          onKeyDown={onHandleKeyDown}
          onBlur={onHandleBlur}
          className={`w-4 h-6 flex-shrink-0 flex items-center justify-center hover:text-[var(--color-text-primary)] focus:text-[var(--color-text-primary)] no-drag cursor-grab active:cursor-grabbing outline-none focus-visible:outline focus-visible:outline-1 focus-visible:outline-[var(--color-accent)] ${
            picked ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-muted)]'
          }`}
        >
          <GripIcon />
        </button>
        <div className="flex-1 min-w-0">
          {editing ? (
            <input
              ref={inputRef}
              aria-label={`New name for ${garden.name}`}
              value={draft}
              disabled={busy}
              maxLength={80}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault()
                  void saveRename()
                } else if (e.key === 'Escape') {
                  e.preventDefault()
                  e.stopPropagation()
                  setEditing(false)
                  setError(null)
                }
              }}
              className={`${INPUT} w-full`}
            />
          ) : (
            <button
              type="button"
              onClick={startRename}
              title="Rename"
              className="max-w-full truncate text-left text-xs text-[var(--color-text-primary)] hover:underline no-drag cursor-pointer"
            >
              {garden.name}
            </button>
          )}
        </div>

        {editing ? (
          <>
            <button type="button" disabled={busy} onClick={() => void saveRename()} className={BTN}>
              Save
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => {
                setEditing(false)
                setError(null)
              }}
              className={BTN}
            >
              Cancel
            </button>
          </>
        ) : (
          <RowMenu
            name={garden.name}
            busy={busy}
            onRename={startRename}
            onDelete={() => {
              setError(null)
              setConfirmDelete(true)
            }}
          />
        )}
      </div>

      {confirmDelete && !editing && (
        <div className="mt-2 flex flex-wrap items-center gap-2" data-zen-garden-confirm={garden.id}>
          <span className="text-[11px] text-[var(--color-text-secondary)]">
            Delete “{garden.name}”? A copy stays in history in the Gardens folder.
          </span>
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void attempt(() => deleteZenSettingsGarden(garden.id), setBusy, setError).then((ok) => {
                if (ok) setConfirmDelete(false)
              })
            }
            className={DANGER_BTN}
          >
            Delete Garden
          </button>
          <button type="button" disabled={busy} onClick={() => setConfirmDelete(false)} className={BTN}>
            Cancel
          </button>
        </div>
      )}
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
 */
function GardenList({ gardens }: { gardens: readonly ZenSettingsGarden[] }): React.JSX.Element {
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
    if (e.button !== 0) return
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
            gardens={gardens}
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
      <div aria-live="polite" role="status" className="sr-only" data-zen-gardens-announce="">
        {announce}
      </div>
    </>
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
      <button type="button" data-zen-settings-new-garden="" onClick={() => setOpen(true)} className={`${BTN} mt-3`}>
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

/** Advanced (collapsed until opened): the global theme and the folder. */
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
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [revealError, setRevealError] = useState<string | null>(null)
  return (
    <section className="mt-8" data-zen-gardens-advanced="" data-settings-id="zen-gardens.advanced">
      <button
        type="button"
        aria-expanded={open}
        aria-controls="zen-gardens-advanced-body"
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1 text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider no-drag cursor-pointer hover:text-[var(--color-text-primary)]"
      >
        <span aria-hidden>{open ? '▾' : '▸'}</span>
        Advanced
      </button>
      {open && (
        <div id="zen-gardens-advanced-body" className="mt-2">
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
      )}
    </section>
  )
}

export function ZenGardensSection(): React.JSX.Element {
  const st = useZenSettingsStore()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    void loadZenSettings()
    return watchLocalZenChanged(() => void loadZenSettings())
  }, [])

  // Before the first answer there is nothing to show but the error, if any.
  const hasData = st.path !== null || st.setUp || st.gardens.length > 0

  return (
    <div className="max-w-2xl" data-zen-gardens-section="">
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
          <GardenList gardens={st.gardens} />
          <NewGarden gardens={st.gardens} />
        </section>
      )}

      {hasData && (st.setUp || st.path) ? (
        <Advanced setUp={st.setUp} path={st.path} themes={st.themes} globalTheme={st.globalTheme} />
      ) : null}
    </div>
  )
}
