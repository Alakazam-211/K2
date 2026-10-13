// prd-garden-lock-and-sync-v1 §3–§5 — Settings → Gardens: two checkboxes
// per Garden row, "Lock garden" and "Keep in sync with future template
// updates". (Replaces the 0.45.1 sync switches, captions, Preview and Keep my
// previous look; the routes and CLI verbs for those stay for support use.)
//
// Thin client over `lib/zen/zen-sync.ts`: the daemon owns each Garden's
// state (`sync.json`); this draws it and posts gestures.
//   - Lock: `POST zen/garden/lock {garden, locked}`. Hidden when this daemon
//     has no Garden lock (its rows carry no `locked`).
//   - Keep in sync: `POST zen/garden/sync {garden, part: "both", sync}`.
//     One part synced and one a copy shows a dash; a click syncs both.
//     Checked (or the dash): a tag to its right names the template the
//     Garden follows (Rosson 2026-10-13). A Garden made from scratch
//     (`k2.blank@1`, or no template) has nothing to follow: the box is off
//     and disabled, with no tag.
// After either change, an 8 s toast with Undo. A daemon without
// `zen-sync-v1` draws nothing.

import React, { useEffect, useRef, useState } from 'react'
import { SquareCheckbox } from '@/components/ui'
import { useZenSettingsStore, zenTemplateLabel } from '@/lib/zen/zen-settings'
import { useZenTemplatesStore } from '@/lib/zen/zen-templates'
import { useToastStore } from '@/stores/toast'
import {
  loadZenSync,
  setZenGardenLock,
  setZenGardenSyncBoth,
  undoZenGardenSync,
  useZenSyncStore,
  zenSyncChecked,
} from '@/lib/zen/zen-sync'

export const ZEN_LOCK_LABEL = 'Lock garden'
export const ZEN_LOCK_TIP = 'Agents can’t change this Garden. You still can.'
export const ZEN_SYNC_LABEL = 'Keep in sync with future template updates'
export const ZEN_SYNC_TIP = 'Gets K2’s improvements to this Garden’s template and theme.'
export const ZEN_SYNC_SCRATCH_TIP = 'Made from scratch, no template to follow'

/** The blank start ("Start empty and ask my agent"): a Garden made from scratch. */
const SCRATCH_TEMPLATE_ID = 'k2.blank@1'

/** Was this Garden made from scratch (no template to follow)? `template` is
 *  the Garden list's template id; not passed (undefined) = not known, not scratch. */
export function zenGardenFromScratch(template: string | undefined): boolean {
  return template !== undefined && (template === '' || template === SCRATCH_TEMPLATE_ID)
}

// Settings → Gardens already re-reads its list on every local `zen_changed`
// (`watchLocalZenChanged` in ZenGardensSection). Rather than hold a second
// socket, the sync state re-reads whenever that list does, so a change here,
// in Zen or from the CLI shows in step.
let users = 0
let unsubscribe: (() => void) | null = null

/** Keep the sync store live while any Garden checkbox is mounted. */
export function useZenSyncLive(): void {
  useEffect(() => {
    users += 1
    if (users === 1) {
      void loadZenSync()
      unsubscribe = useZenSettingsStore.subscribe((s, prev) => {
        if (s.gardens !== prev.gardens || s.setUp !== prev.setUp) void loadZenSync()
      })
    }
    return () => {
      users -= 1
      if (users === 0 && unsubscribe) {
        unsubscribe()
        unsubscribe = null
      }
    }
  }, [])
}

function Check({
  checked,
  label,
  tip,
  disabled,
  dataAttr,
  tag,
  onChange,
}: {
  checked: boolean | 'mixed'
  label: string
  tip: string
  disabled: boolean
  dataAttr: Record<string, string>
  /** A small tag to the right of the label (the template synced with). */
  tag?: React.ReactNode
  onChange(next: boolean): void
}): React.JSX.Element {
  const ref = useRef<HTMLInputElement>(null)
  const mixed = checked === 'mixed'
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = mixed
  }, [mixed])
  return (
    <label
      title={tip}
      className={`flex items-center gap-2 py-0.5 text-xs text-[var(--color-text-primary)] no-drag ${
        disabled ? 'cursor-default' : 'cursor-pointer'
      }`}
      {...dataAttr}
    >
      <SquareCheckbox
        ref={ref}
        checked={checked === true}
        aria-checked={mixed ? 'mixed' : checked}
        aria-label={label}
        disabled={disabled}
        // A dash (one part synced, one a copy): one click syncs both.
        onChange={(e) => {
          if (!disabled) onChange(mixed ? true : e.target.checked)
        }}
      />
      <span>{label}</span>
      {tag}
    </label>
  )
}

/** The two checkboxes for one Garden row. Nothing when this daemon has no
 *  Garden sync, or before its first answer. */
export function ZenGardenSyncControls({
  garden,
}: {
  /** `template`: the Garden list's template id (where the Garden came from). */
  garden: { id: string; name: string; template?: string }
}): React.JSX.Element | null {
  useZenSyncLive()
  // Re-render when the catalog arrives, so a catalog Garden's tag reads its label.
  useZenTemplatesStore((s) => s.templates)
  const row = useZenSyncStore((s) => s.list?.gardens.find((g) => g.id === garden.id) ?? null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  if (!row) return null

  const run = (what: () => Promise<void>, done: string, undo: () => Promise<void>): void => {
    setBusy(true)
    setError(null)
    what()
      .then(() => {
        useToastStore.getState().addToast(done, 'success', 8000, {
          label: 'Undo',
          onClick: () => void undo().catch((e: unknown) => setError(e instanceof Error ? e.message : String(e))),
        })
      })
      .catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setBusy(false))
  }

  const scratch = zenGardenFromScratch(garden.template)
  const synced = scratch ? false : zenSyncChecked(row)
  const templateLabel = garden.template ? zenTemplateLabel(garden.template) : null
  const tag =
    synced !== false && templateLabel ? (
      <span
        data-zen-garden-sync-template={garden.id}
        className="flex-shrink-0 px-1 py-0.5 text-[9px] leading-none text-[var(--color-text-muted)] border border-[var(--color-border)]"
      >
        {templateLabel}
      </span>
    ) : null
  return (
    <div className="mt-1" data-zen-sync-controls={garden.id}>
      {row.locked !== null && (
        <Check
          checked={row.locked}
          label={ZEN_LOCK_LABEL}
          tip={ZEN_LOCK_TIP}
          disabled={busy}
          dataAttr={{ 'data-zen-garden-lock': garden.id }}
          onChange={(next) =>
            run(
              () => setZenGardenLock(garden.id, next),
              next ? `${row.name} is locked` : `${row.name} is unlocked`,
              () => setZenGardenLock(garden.id, !next),
            )
          }
        />
      )}
      <Check
        checked={synced}
        label={ZEN_SYNC_LABEL}
        tip={scratch ? ZEN_SYNC_SCRATCH_TIP : ZEN_SYNC_TIP}
        disabled={busy || scratch}
        dataAttr={{ 'data-zen-garden-sync': garden.id }}
        tag={tag}
        onChange={(next) =>
          run(
            () => setZenGardenSyncBoth(garden.id, next),
            next ? `${row.name} now keeps in sync` : `${row.name} no longer syncs`,
            () => undoZenGardenSync(garden.id),
          )
        }
      />
      {error && (
        <p role="alert" className="text-[11px] text-[var(--color-status-error)] mt-1">
          {error}
        </p>
      )}
    </div>
  )
}
