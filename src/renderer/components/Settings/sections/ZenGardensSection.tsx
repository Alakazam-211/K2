// Settings → Gardens (prd-zen-gardens-v1 item 17; prd-garden-lock-and-sync-v1
// §3, Rosson 2026-10-13). This computer's Zen Gardens, one row each: the
// name, a ⋯ menu (Rename, Move up, Move down, Delete) and two checkboxes,
// "Lock garden" and "Keep in sync with future template updates"
// (`ZenGardenSyncControls`). + New Garden opens the same modal as Zen's. A
// collapsed Advanced holds the global theme and the folder. Thin client over
// `lib/zen/zen-settings.ts`: every request goes to the LOCAL daemon. Re-reads
// on every local `zen_changed`, like the Garden switcher, so Settings and an
// open Zen window stay in step.
//
// Settings only shows this where the Zen toggle shows
// (`useZenGardensSettingsShown`); the web client never mounts it.

import React, { useEffect, useRef, useState } from 'react'
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
  moveZenSettingsGarden,
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

/** A row's ⋯ menu: Rename, Move up, Move down, Delete. */
function RowMenu({
  name,
  busy,
  canUp,
  canDown,
  onRename,
  onUp,
  onDown,
  onDelete,
}: {
  name: string
  busy: boolean
  canUp: boolean
  canDown: boolean
  onRename(): void
  onUp(): void
  onDown(): void
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
          <button type="button" role="menuitem" className={MENU_ITEM} disabled={!canUp} onClick={pick(onUp)}>
            Move up
          </button>
          <button type="button" role="menuitem" className={MENU_ITEM} disabled={!canDown} onClick={pick(onDown)}>
            Move down
          </button>
          <button type="button" role="menuitem" className={MENU_ITEM} onClick={pick(onDelete)}>
            Delete
          </button>
        </div>
      )}
    </div>
  )
}

function GardenRow({
  garden,
  gardens,
}: {
  garden: ZenSettingsGarden
  gardens: readonly ZenSettingsGarden[]
}): React.JSX.Element {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(garden.name)
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const count = gardens.length
  const at = garden.index

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
    <li data-zen-garden-row={garden.id} className="py-2 border-b border-[var(--color-border)]">
      <div className="flex items-center gap-2">
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
            canUp={at > 1}
            canDown={at < count}
            onRename={startRename}
            onUp={() => void attempt(() => moveZenSettingsGarden(garden.id, at - 1), setBusy, setError)}
            onDown={() => void attempt(() => moveZenSettingsGarden(garden.id, at + 1), setBusy, setError)}
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
          <ol className="border-t border-[var(--color-border)]">
            {st.gardens.map((g) => (
              <GardenRow key={g.id} garden={g} gardens={st.gardens} />
            ))}
          </ol>
          <NewGarden gardens={st.gardens} />
        </section>
      )}

      {hasData && (st.setUp || st.path) ? (
        <Advanced setUp={st.setUp} path={st.path} themes={st.themes} globalTheme={st.globalTheme} />
      ) : null}
    </div>
  )
}
