// Settings → Gardens (prd-zen-gardens-v1 item 17, Rosson 2026-10-04).
// This computer's Zen Gardens: list, rename, reorder, delete, + New Garden
// (default layout or empty), the global theme with per-Garden overrides, and
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
import { watchLocalZenChanged } from '@/lib/zen/zen-api'
import { currentDesktopOs } from '@/lib/zen/zen-platform'
import {
  checkZenGardenName,
  createZenSettingsGarden,
  deleteZenSettingsGarden,
  loadZenSettings,
  moveZenSettingsGarden,
  nextZenGardenName,
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
import { useZenCatalogBadges, useZenTemplatesStore, zenCatalogGardenName, zenTemplateSections } from '@/lib/zen/zen-templates'
import {
  ZEN_TEMPLATES_FALLBACK,
  type ZenGardenNewRequest,
  type ZenScope,
  type ZenTemplateInfo,
  type ZenWidgetGrantRow,
} from '@/lib/zen/zen-custom-types'
import { zenScopeKind, zenScopeRows, zenScopeWhere } from '@/lib/zen/zen-custom-scope'
import { K2_CAPS } from '@/lib/k2-caps.generated'
import {
  ZenCapList,
  ZenScopeCount,
  ZenScopePicker,
  ZenSendingChoice,
  zenDefaultScope,
} from '@/components/Zen/widgets/ZenGrantDialog'

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
      <div className="ml-12">
        <InlineError text={error} testId={`zen-garden-error-${garden.id}`} />
      </div>
    </li>
  )
}

function NewGarden({ gardens }: { gardens: readonly ZenSettingsGarden[] }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // A ready-made Garden that runs a widget: its scope, picked here (UWB22).
  const [entry, setEntry] = useState<ZenTemplateInfo | null>(null)
  const [scope, setScope] = useState<ZenScope | null>(null)
  const [sending, setSending] = useState(true)
  const [confirmed, setConfirmed] = useState(false)
  const templates = useZenTemplatesStore((s) => s.templates)
  const fallback = nextZenGardenName(gardens)

  const create = async (template: ZenNewGardenTemplate, grant?: ZenGardenNewRequest['grant']): Promise<void> => {
    let clean: string
    try {
      clean = checkZenGardenName(name.trim() === '' ? fallback : name, gardens)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      return
    }
    const ok = await attempt(() => createZenSettingsGarden(clean, template, grant), setBusy, setError)
    if (ok) {
      setName('')
      setOpen(false)
      setEntry(null)
    }
  }
  const pickCatalog = (t: ZenTemplateInfo): void => {
    if (!t.needsGrant) {
      void create(t.short)
      return
    }
    setEntry(t)
    setScope(zenDefaultScope())
    setSending(true)
    setConfirmed(false)
    setError(null)
  }
  const { starts, catalog } = zenTemplateSections(templates)

  if (!open) {
    return (
      <button
        type="button"
        onClick={() => {
          setError(null)
          setOpen(true)
        }}
        className={`${BTN} mt-3`}
      >
        + New Garden
      </button>
    )
  }

  return (
    <div className="mt-3 p-3 border border-[var(--color-border)] space-y-2" data-zen-new-garden="">
      <input
        aria-label="New Garden name"
        autoFocus
        value={name}
        placeholder={fallback}
        disabled={busy}
        maxLength={80}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.preventDefault()
            e.stopPropagation()
            setOpen(false)
          }
        }}
        className={`${INPUT} w-full`}
      />
      {entry?.needsGrant ? (
        <CatalogGrant
          entry={entry}
          scope={scope}
          onScope={setScope}
          sending={sending}
          onSending={setSending}
          confirmed={confirmed}
          onConfirmed={setConfirmed}
          busy={busy}
          onBack={() => setEntry(null)}
          onCreate={(grant) => void create(entry.short, grant)}
        />
      ) : (
        <>
          <div className="flex flex-wrap gap-2">
            {(starts.length > 0 ? starts : ZEN_TEMPLATES_FALLBACK).map((t) => (
              <button key={t.short} type="button" disabled={busy} onClick={() => void create(t.short)} className={BTN}>
                {t.label}
              </button>
            ))}
            <button type="button" disabled={busy} onClick={() => setOpen(false)} className={BTN}>
              Cancel
            </button>
          </div>
          {catalog.length > 0 && (
            <div className="space-y-1" data-zen-settings-catalog="">
              <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
                Ready-made Gardens
              </div>
              <div className="flex flex-wrap gap-2">
                {catalog.map((t) => (
                  <button
                    key={t.short}
                    type="button"
                    disabled={busy}
                    title={t.description}
                    data-zen-settings-catalog-choice={t.short}
                    onClick={() => pickCatalog(t)}
                    className={BTN}
                  >
                    {t.label}
                    <CatalogBadge short={t.short} />
                  </button>
                ))}
              </div>
            </div>
          )}
        </>
      )}
      <InlineError text={error} testId="zen-new-garden-error" />
    </div>
  )
}

/** Zen's own tokens mapped onto Settings' look, so the shared scope picker
 *  and cap list (drawn with `--zen-*`) read as part of this page. */
const ZEN_TOKENS_IN_SETTINGS = {
  '--zen-text': 'var(--color-text-primary)',
  '--zen-text-muted': 'var(--color-text-muted)',
  '--zen-surface': 'var(--color-bg)',
  '--zen-surface-raised': 'var(--color-bg-elevated)',
  '--zen-border': 'var(--color-border)',
  '--zen-accent': 'var(--color-accent)',
  '--zen-danger': 'var(--color-status-error)',
  '--zen-radius': '6px',
} as React.CSSProperties

/** UWB22: which agents a ready-made Garden's widget may see, in the same click. */
function CatalogGrant({
  entry,
  scope,
  onScope,
  sending,
  onSending,
  confirmed,
  onConfirmed,
  busy,
  onBack,
  onCreate,
}: {
  entry: ZenTemplateInfo
  scope: ZenScope | null
  onScope(s: ZenScope | null): void
  sending: boolean
  onSending(on: boolean): void
  confirmed: boolean
  onConfirmed(on: boolean): void
  busy: boolean
  onBack(): void
  onCreate(grant: { scope: ZenScope; sending: boolean }): void
}): React.JSX.Element {
  const caps = entry.needsGrant?.caps ?? []
  const posts = caps.includes('thread:post')
  const everyServer = scope !== null && zenScopeKind(scope) === 'allServers'
  const empty = scope === null || zenScopeRows(scope).rows.length === 0
  const blocked = busy || empty || (posts && sending && everyServer && !confirmed)
  return (
    <div className="space-y-2 text-xs" style={ZEN_TOKENS_IN_SETTINGS} data-zen-settings-catalog-grant={entry.short}>
      <div className="text-[var(--color-text-primary)]">
        <span className="font-semibold">{entry.label}</span>: its page can
      </div>
      <ZenCapList caps={caps} scope={scope} />
      <ZenScopePicker ask={{}} value={scope} onChange={onScope} />
      <ZenScopeCount scope={scope} />
      {posts && (
        <ZenSendingChoice
          sending={sending}
          onSending={onSending}
          everyServer={everyServer}
          confirmed={confirmed}
          onConfirmed={onConfirmed}
        />
      )}
      <div className="flex flex-wrap gap-2">
        <button type="button" disabled={busy} onClick={onBack} className={BTN}>
          Back
        </button>
        <button
          type="button"
          disabled={blocked}
          data-zen-settings-catalog-create=""
          onClick={() => scope && onCreate({ scope, sending: posts ? sending : false })}
          className={BTN}
        >
          Create
        </button>
      </div>
    </div>
  )
}

/** A catalog entry's badge ("New"), set by whoever knows what's new. */
function CatalogBadge({ short }: { short: string }): React.JSX.Element | null {
  const text = useZenCatalogBadges((s) => s.badges[short])
  if (!text) return null
  return (
    <span
      data-zen-catalog-badge={short}
      className="ml-2 px-1.5 py-px text-[9px] font-semibold uppercase tracking-wider text-[var(--color-accent)] border border-[var(--color-accent)]"
    >
      {text}
    </span>
  )
}

/** A catalog Garden's preview: a small page sketch in its own colours. The
 *  catalog rows carry no picture yet, so K2 draws one from the entry. */
function CatalogPreview({ entry, large }: { entry: ZenTemplateInfo; large?: boolean }): React.JSX.Element {
  // A stable hue per Garden, so each entry keeps its look.
  let hash = 0
  for (const ch of entry.short) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0
  const hue = hash % 360
  return (
    <div
      aria-hidden
      data-zen-catalog-preview={entry.short}
      className="relative w-full overflow-hidden border border-[var(--color-border)]"
      style={{
        aspectRatio: large ? '16 / 7' : '16 / 9',
        background: `linear-gradient(135deg, hsl(${hue} 45% 92%), hsl(${(hue + 40) % 360} 40% 80%))`,
      }}
    >
      <div className="absolute inset-0 flex" style={{ padding: large ? 14 : 8, gap: large ? 10 : 6 }}>
        <div style={{ flex: '1 1 0%', background: 'rgba(255,255,255,0.55)', borderRadius: 4 }} />
        <div className="flex flex-col" style={{ flex: '2 1 0%', gap: large ? 8 : 4 }}>
          <div style={{ height: large ? 16 : 8, width: '60%', background: `hsl(${hue} 35% 35% / 0.55)`, borderRadius: 3 }} />
          <div style={{ flex: '1 1 0%', background: 'rgba(255,255,255,0.7)', borderRadius: 4 }} />
        </div>
      </div>
      <span
        className="absolute font-semibold"
        style={{ right: 8, bottom: 4, fontSize: large ? 28 : 18, color: `hsl(${hue} 40% 30% / 0.7)` }}
      >
        {entry.label.slice(0, 1).toUpperCase()}
      </span>
    </div>
  )
}

/**
 * Settings → Gardens → Garden catalog (Rosson 2026-10-08): browse the
 * ready-made Gardens, see one up close, and Add it as a new Garden. The
 * list is the daemon's (`GET /cli/zen/templates`, section `catalog`), so a
 * new catalog Garden shows here with no code change. Adding makes a NEW
 * Garden; the person's Gardens never change. One that runs a widget asks
 * which agents it may see in the same click (UWB22).
 */
function GardenCatalog({ gardens }: { gardens: readonly ZenSettingsGarden[] }): React.JSX.Element | null {
  const templates = useZenTemplatesStore((s) => s.templates)
  const catalog = zenTemplateSections(templates).catalog
  const [picked, setPicked] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const [name, setName] = useState('')
  const [scope, setScope] = useState<ZenScope | null>(null)
  const [sending, setSending] = useState(true)
  const [confirmed, setConfirmed] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  if (catalog.length === 0) return null
  const entry = catalog.find((t) => t.short === picked) ?? null
  const startAdd = (t: ZenTemplateInfo): void => {
    setAdding(true)
    setName(zenCatalogGardenName(t.label, gardens.map((g) => g.name)))
    setScope(zenDefaultScope())
    setSending(true)
    setConfirmed(false)
    setError(null)
  }
  const create = async (t: ZenTemplateInfo, grant?: ZenGardenNewRequest['grant']): Promise<void> => {
    let clean: string
    try {
      clean = checkZenGardenName(name, gardens)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      return
    }
    if (await attempt(() => createZenSettingsGarden(clean, t.short, grant), setBusy, setError)) setAdding(false)
  }
  const inUse = entry ? gardens.filter((g) => g.template === entry.id).length : 0
  return (
    <section data-settings-id="zen-gardens.catalog" className="mt-8" data-zen-settings-catalog-area="">
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1">
        Garden catalog
      </h3>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-3">
        Ready-made Gardens from K2. Adding one makes a new Garden; your Gardens stay as they are.
      </p>
      <div className="grid grid-cols-2 gap-3">
        {catalog.map((t) => (
          <button
            key={t.short}
            type="button"
            aria-pressed={picked === t.short}
            data-zen-catalog-card={t.short}
            onClick={() => {
              setPicked(t.short)
              setAdding(false)
              setError(null)
            }}
            className={`text-left p-2 border transition-colors no-drag cursor-pointer hover:bg-[var(--color-bg-elevated)] ${
              picked === t.short ? 'border-[var(--color-accent)]' : 'border-[var(--color-border)]'
            }`}
          >
            <CatalogPreview entry={t} />
            <div className="mt-2 flex items-center text-xs text-[var(--color-text-primary)]">
              <span className="truncate">{t.label}</span>
              <CatalogBadge short={t.short} />
            </div>
            <div className="text-[10px] text-[var(--color-text-muted)] line-clamp-2">{t.description}</div>
          </button>
        ))}
      </div>
      {entry && (
        <div className="mt-3 p-3 border border-[var(--color-border)] space-y-2 text-xs" data-zen-catalog-detail={entry.short}>
          <CatalogPreview entry={entry} large />
          <div className="flex items-center text-[var(--color-text-primary)]">
            <span className="font-semibold">{entry.label}</span>
            <CatalogBadge short={entry.short} />
          </div>
          <p className="text-[var(--color-text-secondary)]">{entry.description}</p>
          {entry.needsGrant && entry.needsGrant.caps.length > 0 && (
            <div style={ZEN_TOKENS_IN_SETTINGS} className="space-y-1">
              <div className="text-[var(--color-text-muted)]">Its page asks to:</div>
              <ZenCapList caps={entry.needsGrant.caps} scope={null} />
            </div>
          )}
          {inUse > 0 && (
            <p className="text-[10px] text-[var(--color-text-muted)]" data-zen-catalog-in-use={inUse}>
              You have {inUse === 1 ? 'one Garden' : `${inUse} Gardens`} from it already.
            </p>
          )}
          {!adding ? (
            <button type="button" data-zen-catalog-add="" disabled={busy} onClick={() => startAdd(entry)} className={BTN}>
              Add
            </button>
          ) : (
            <div className="space-y-2" data-zen-catalog-adding="">
              <input
                aria-label="Name for the new Garden"
                value={name}
                disabled={busy}
                maxLength={80}
                onChange={(e) => setName(e.target.value)}
                className={`${INPUT} w-full`}
              />
              {entry.needsGrant ? (
                <CatalogGrant
                  entry={entry}
                  scope={scope}
                  onScope={setScope}
                  sending={sending}
                  onSending={setSending}
                  confirmed={confirmed}
                  onConfirmed={setConfirmed}
                  busy={busy}
                  onBack={() => setAdding(false)}
                  onCreate={(grant) => void create(entry, grant)}
                />
              ) : (
                <div className="flex gap-2">
                  <button type="button" disabled={busy} onClick={() => setAdding(false)} className={BTN}>
                    Cancel
                  </button>
                  <button type="button" disabled={busy} data-zen-catalog-create="" onClick={() => void create(entry)} className={BTN}>
                    Create
                  </button>
                </div>
              )}
            </div>
          )}
          <InlineError text={error} testId="zen-catalog-error" />
        </div>
      )}
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
  const gardenName = (id: string): string => gardens.find((g) => g.id === id)?.name ?? 'a deleted Garden'
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
                      {g.widget} <span className="text-[var(--color-text-muted)]">in {gardenName(g.garden)}</span>
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
