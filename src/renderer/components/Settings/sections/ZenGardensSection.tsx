// Settings → Gardens (prd-zen-gardens-v1 item 17, Rosson 2026-10-04).
// This computer's Zen Gardens: list, rename, reorder, delete, + New Garden
// (the same modal as Zen's: a name and the catalog's cards, Rosson
// 2026-10-08), the global theme with per-Garden overrides, and
// the folder. Thin client over `lib/zen/zen-settings.ts`: every request goes
// to the LOCAL daemon. Re-reads on every local `zen_changed`, like the
// Garden switcher, so Settings and an open Zen window stay in step.
//
// Settings only shows this where the Zen toggle shows
// (`useZenGardensSettingsShown`); the web client never mounts it.

import React, { useEffect, useRef, useState } from 'react'
import { revealItemInDir } from '@tauri-apps/plugin-opener'
import type { SettingEntry } from '../searchManifest'
import { SettingDropdown } from '../controls/SettingControls'
import { ZenGardenSettingsNews, ZenGardenSyncControls } from './ZenGardenSyncControls'
import { watchLocalZenChanged } from '@/lib/zen/zen-api'
import { currentDesktopOs } from '@/lib/zen/zen-platform'
import {
  checkZenGardenName,
  createZenSettingsGarden,
  deleteZenSettingsGarden,
  loadZenSettings,
  moveZenSettingsGarden,
  renameZenSettingsGarden,
  setZenGardenTheme,
  setZenGlobalTheme,
  setupZenGardens,
  useZenSettingsStore,
  zenSettingsErrorText,
  zenTemplateLabel,
  resumeZenSettingsGrant,
  revokeZenSettingsGrant,
  setZenSettingsGrantSending,
  type ZenNewGardenTemplate,
  type ZenSettingsGarden,
  type ZenSettingsTheme,
} from '@/lib/zen/zen-settings'
import { useZenCatalogBadges, useZenTemplatesStore, zenTemplateSections } from '@/lib/zen/zen-templates'
import { type ZenWidgetGrantRow } from '@/lib/zen/zen-custom-types'
import { zenScopeWhere } from '@/lib/zen/zen-custom-scope'
import { K2_CAPS } from '@/lib/k2-caps.generated'
import { ZenGardenSketch, ZenNewGardenModal } from '@/components/Zen/widgets/ZenNewGardenModal'

export const ZEN_GARDENS_MANIFEST: SettingEntry[] = [
  {
    id: 'zen-gardens.list',
    section: 'zen-gardens',
    label: 'Gardens',
    description: 'Rename, reorder or delete your Zen Gardens',
    keywords: ['zen', 'garden', 'gardens', 'rename', 'reorder', 'delete', 'new garden'],
  },
  {
    id: 'zen-gardens.theme',
    section: 'zen-gardens',
    label: 'Zen Theme',
    description: 'The theme every Garden uses, and per-Garden overrides',
    keywords: ['zen', 'theme', 'garden', 'appearance'],
  },
  {
    id: 'zen-gardens.catalog',
    section: 'zen-gardens',
    label: 'Garden Catalog',
    description: 'Browse ready-made Gardens (like the Diary) and add one',
    keywords: ['zen', 'garden', 'catalog', 'diary', 'ready-made', 'template', 'add'],
  },
  {
    id: 'zen-gardens.widgets',
    section: 'zen-gardens',
    label: 'Widget Permissions',
    description: 'What you allowed custom Garden widgets: turn sending or a widget off',
    keywords: ['zen', 'widget', 'permission', 'grant', 'sending', 'revoke'],
  },
  {
    id: 'zen-gardens.folder',
    section: 'zen-gardens',
    label: 'Gardens Folder',
    description: 'Where Gardens live on this computer (~/.k2/zen)',
    keywords: ['zen', 'folder', 'path', 'finder'],
  },
]

const BTN =
  'px-2 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default'
const ICON_BTN =
  'w-6 h-6 flex items-center justify-center text-xs border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-30 disabled:cursor-default'
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

function GardenRow({
  garden,
  gardens,
  themes,
  globalTheme,
}: {
  garden: ZenSettingsGarden
  gardens: readonly ZenSettingsGarden[]
  themes: readonly ZenSettingsTheme[]
  globalTheme: string | null
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

  const followLabel = globalTheme ? `Follow global (${globalTheme})` : 'Follow global'

  return (
    <li data-zen-garden-row={garden.id} className="py-2 border-b border-[var(--color-border)]">
      <div className="flex items-center gap-2">
        <span className="w-5 text-right text-[11px] tabular-nums text-[var(--color-text-muted)]" title="Position">
          {at}
        </span>
        <div className="flex flex-col">
          <button
            type="button"
            aria-label={`Move ${garden.name} up`}
            title="Move up"
            disabled={busy || at <= 1}
            onClick={() => void attempt(() => moveZenSettingsGarden(garden.id, at - 1), setBusy, setError)}
            className={`${ICON_BTN} h-3 border-b-0`}
          >
            <svg width="8" height="8" viewBox="0 0 10 10" fill="currentColor" aria-hidden>
              <path d="M5 2 9 8H1z" />
            </svg>
          </button>
          <button
            type="button"
            aria-label={`Move ${garden.name} down`}
            title="Move down"
            disabled={busy || at >= count}
            onClick={() => void attempt(() => moveZenSettingsGarden(garden.id, at + 1), setBusy, setError)}
            className={`${ICON_BTN} h-3`}
          >
            <svg width="8" height="8" viewBox="0 0 10 10" fill="currentColor" aria-hidden>
              <path d="M5 8 1 2h8z" />
            </svg>
          </button>
        </div>

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
          <div className="text-[10px] text-[var(--color-text-muted)]">{zenTemplateLabel(garden.template)}</div>
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
          <>
            <SettingDropdown
              ariaLabel={`Theme for ${garden.name}`}
              value={garden.theme ?? ''}
              options={[{ value: '', label: followLabel }, ...themeOptions(themes)]}
              placeholder={garden.theme ?? followLabel}
              disabled={busy || themes.length === 0}
              onChange={(v) =>
                void attempt(() => setZenGardenTheme(garden.id, v === '' ? null : v), setBusy, setError)
              }
            />
            <button type="button" disabled={busy} onClick={startRename} className={BTN}>
              Rename
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => {
                setError(null)
                setConfirmDelete(true)
              }}
              className={BTN}
            >
              Delete
            </button>
          </>
        )}
      </div>

      {confirmDelete && !editing && (
        <div className="mt-2 ml-12 flex flex-wrap items-center gap-2" data-zen-garden-confirm={garden.id}>
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
      <div className="ml-12">
        <InlineError text={error} testId={`zen-garden-error-${garden.id}`} />
      </div>
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
 * name and the catalog's cards, the Diary first. `highlight` opens it on
 * a catalog entry (the Garden catalog's cards). Creates through the local
 * daemon; a catalog Garden's grant rides the same click.
 */
function SettingsNewGardenModal({
  gardens,
  highlight,
  onClose,
}: {
  gardens: readonly ZenSettingsGarden[]
  highlight: string | null
  onClose(): void
}): React.JSX.Element {
  return (
    <ZenNewGardenModal
      taken={gardens.map((g) => g.name)}
      highlight={highlight}
      vars={ZEN_TOKENS_IN_SETTINGS}
      onClose={onClose}
      onCreate={async (pick) => {
        const clean = checkZenGardenName(pick.name, gardens)
        await createZenSettingsGarden(clean, pick.entry.short as ZenNewGardenTemplate, pick.grant)
      }}
    />
  )
}

function NewGarden({ gardens }: { gardens: readonly ZenSettingsGarden[] }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button type="button" data-zen-settings-new-garden="" onClick={() => setOpen(true)} className={`${BTN} mt-3`}>
        + New Garden
      </button>
      {open && <SettingsNewGardenModal gardens={gardens} highlight={null} onClose={() => setOpen(false)} />}
    </>
  )
}

/**
 * Settings → Gardens → Garden catalog (Rosson 2026-10-08): the ready-made
 * Gardens as the same cards New Garden shows. The list is the daemon's
 * (`GET /cli/zen/templates`, section `catalog`), so a new catalog Garden
 * shows here with no code change. Picking a card opens New Garden on it;
 * Create makes a NEW Garden (the person's Gardens never change), and for
 * K2's own Gardens the click is the consent, said in one sentence there.
 */
function GardenCatalog({ gardens }: { gardens: readonly ZenSettingsGarden[] }): React.JSX.Element | null {
  const templates = useZenTemplatesStore((s) => s.templates)
  const badges = useZenCatalogBadges((s) => s.badges)
  const catalog = zenTemplateSections(templates).catalog
  const [adding, setAdding] = useState<string | null>(null)
  if (catalog.length === 0) return null
  return (
    <section data-settings-id="zen-gardens.catalog" className="mt-8" data-zen-settings-catalog-area="">
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1">
        Garden catalog
      </h3>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-3">
        Ready-made Gardens from K2. Adding one makes a new Garden; your Gardens stay as they are.
      </p>
      <div className="grid grid-cols-2 gap-3" style={ZEN_TOKENS_IN_SETTINGS}>
        {catalog.map((t) => {
          const inUse = gardens.filter((g) => g.template === t.id).length
          return (
            <button
              key={t.short}
              type="button"
              data-zen-catalog-card={t.short}
              onClick={() => setAdding(t.short)}
              className="text-left p-2 border border-[var(--color-border)] transition-colors no-drag cursor-pointer hover:bg-[var(--color-bg-elevated)] flex flex-col gap-1"
            >
              <ZenGardenSketch entry={t} />
              <div className="mt-1 flex items-center text-xs text-[var(--color-text-primary)]">
                <span className="truncate">{t.label}</span>
                {badges[t.short] && (
                  <span
                    data-zen-catalog-badge={t.short}
                    className="ml-2 px-1.5 py-px text-[9px] font-semibold uppercase tracking-wider text-[var(--color-accent)] border border-[var(--color-accent)]"
                  >
                    {badges[t.short]}
                  </span>
                )}
              </div>
              <div className="text-[10px] text-[var(--color-text-muted)] line-clamp-2">{t.description}</div>
              {inUse > 0 && (
                <div className="text-[10px] text-[var(--color-text-muted)]" data-zen-catalog-in-use={inUse}>
                  You have {inUse === 1 ? 'one Garden' : `${inUse} Gardens`} from it already.
                </div>
              )}
            </button>
          )
        })}
      </div>
      {adding && <SettingsNewGardenModal gardens={gardens} highlight={adding} onClose={() => setAdding(null)} />}
    </section>
  )
}

/** Settings → Gardens → Widget permissions (UWB3: the grants list). */
function WidgetPermissions({
  grants,
  error,
  gardens,
}: {
  grants: ZenWidgetGrantRow[]
  error: string | null
  gardens: readonly ZenSettingsGarden[]
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const gardenName = (g: ZenWidgetGrantRow): string =>
    gardens.find((x) => x.id === g.garden)?.name ?? g.gardenName ?? 'a deleted Garden'
  return (
    <section data-settings-id="zen-gardens.widgets" className="mt-8" data-zen-settings-grants={grants.length}>
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1">
        Widget permissions
      </h3>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-2">
        What you allowed widgets that agents wrote for your Gardens. Only you can allow one, from its Garden.
      </p>
      <InlineError text={error} testId="zen-grants-load-error" />
      {grants.length === 0 && !error ? (
        <p className="text-xs text-[var(--color-text-muted)]" data-zen-settings-grants-empty="">
          No widget has permissions.
        </p>
      ) : (
        <ul className="border-t border-[var(--color-border)]">
          {grants.map((g) => {
            const key = `${g.garden}/${g.placement}`
            const canSend = g.caps.includes('thread:post')
            return (
              <li key={key} data-zen-settings-grant={key} className="py-2 border-b border-[var(--color-border)] space-y-1">
                <div className="flex items-center justify-between gap-3">
                  <div className="min-w-0">
                    <div className="text-xs text-[var(--color-text-primary)] truncate">
                      {g.widget} <span className="text-[var(--color-text-muted)]">in {gardenName(g)}</span>
                    </div>
                    <div className="text-[10px] text-[var(--color-text-muted)]">
                      {zenScopeWhere(g.scope)} · {g.caps.map((c) => K2_CAPS[c].label).join(', ') || 'no permissions'}
                      {g.state === 'invalid' ? ' · K2 couldn’t confirm it' : g.state === 'review' ? ' · needs a new review' : ''}
                      {g.paused ? ' · paused by K2' : ''}
                    </div>
                  </div>
                  <div className="flex flex-shrink-0 items-center gap-2">
                    {g.paused && (
                      <button
                        type="button"
                        disabled={busy}
                        data-zen-settings-grant-resume=""
                        onClick={() => void attempt(() => resumeZenSettingsGrant(g.garden, g.placement), setBusy, setActionError)}
                        className={BTN}
                      >
                        Resume
                      </button>
                    )}
                    {canSend && (
                      <button
                        type="button"
                        disabled={busy}
                        data-zen-settings-grant-sending={g.sending ? 'on' : 'off'}
                        onClick={() =>
                          void attempt(() => setZenSettingsGrantSending(g.garden, g.placement, !g.sending), setBusy, setActionError)
                        }
                        className={BTN}
                      >
                        {g.sending ? 'Sending on' : 'Sending off'}
                      </button>
                    )}
                    <button
                      type="button"
                      disabled={busy}
                      data-zen-settings-grant-revoke=""
                      onClick={() => void attempt(() => revokeZenSettingsGrant(g.garden, g.placement), setBusy, setActionError)}
                      className={DANGER_BTN}
                    >
                      Turn off
                    </button>
                  </div>
                </div>
              </li>
            )
          })}
        </ul>
      )}
      <InlineError text={actionError} testId="zen-grants-error" />
    </section>
  )
}

export function ZenGardensSection(): React.JSX.Element {
  const st = useZenSettingsStore()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [revealError, setRevealError] = useState<string | null>(null)

  useEffect(() => {
    void loadZenSettings()
    return watchLocalZenChanged(() => void loadZenSettings())
  }, [])

  // Before the first answer there is nothing to show but the error, if any.
  const hasData = st.path !== null || st.setUp || st.gardens.length > 0

  return (
    <div className="max-w-2xl" data-zen-gardens-section="">
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Gardens</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-6">
        Your Zen Mode pages. They live on this computer and only you see them.
      </p>

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
          <p className="text-xs text-[var(--color-text-secondary)] mb-3">
            Gardens aren’t set up on this computer yet.
          </p>
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
        <>
          <section data-settings-id="zen-gardens.list">
            <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1">
              Your Gardens
            </h3>
            <ZenGardenSettingsNews />
            <ol className="border-t border-[var(--color-border)]">
              {st.gardens.map((g) => (
                <GardenRow
                  key={g.id}
                  garden={g}
                  gardens={st.gardens}
                  themes={st.themes}
                  globalTheme={st.globalTheme}
                />
              ))}
            </ol>
            <NewGarden gardens={st.gardens} />
          </section>

          <GardenCatalog gardens={st.gardens} />

          <section data-settings-id="zen-gardens.theme" className="mt-8">
            <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1">
              Theme
            </h3>
            <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
              <div>
                <div className="text-xs text-[var(--color-text-primary)]">Global theme</div>
                <div className="text-[10px] text-[var(--color-text-muted)]">
                  Every Garden uses it unless the Garden picks its own above.
                </div>
              </div>
              <SettingDropdown
                ariaLabel="Global Zen theme"
                value={st.globalTheme ?? ''}
                options={themeOptions(st.themes)}
                placeholder={st.globalTheme ?? 'No theme'}
                disabled={busy || st.themes.length === 0}
                onChange={(v) => void attempt(() => setZenGlobalTheme(v), setBusy, setError)}
              />
            </div>
            <InlineError text={error} testId="zen-gardens-theme-error" />
          </section>

          {st.grants !== null && <WidgetPermissions grants={st.grants} error={st.grantsError} gardens={st.gardens} />}
        </>
      )}

      {st.path ? (
        <section data-settings-id="zen-gardens.folder" className="mt-8">
          <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1">
            Folder
          </h3>
          <div className="flex items-center justify-between gap-3 py-2 border-b border-[var(--color-border)]">
            <span className="text-[11px] font-mono text-[var(--color-text-primary)] select-all truncate" data-zen-gardens-path="">
              {st.path}
            </span>
            {st.setUp ? (
              <button
                type="button"
                onClick={() => {
                  setRevealError(null)
                  revealItemInDir(st.path as string).catch((err: unknown) =>
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
        </section>
      ) : null}
    </div>
  )
}
