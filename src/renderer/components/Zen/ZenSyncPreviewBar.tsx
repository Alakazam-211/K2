// prd-zen-garden-sync-defaults-v1 GS34 — K2's bar above a Garden while
// Settings previews K2's default for it: "Previewing K2's default for
// Garden 2 · Turn sync on · Back to my copy". Drawn by K2 outside the Garden
// page, like the safe-mode banner (`ZenBanners.tsx`).
//
// The preview itself is `preview=` on `GET /cli/zen/get` (zen-api.ts reads
// `useZenSyncPreviewStore`), which writes nothing. Leaving Zen or switching
// Gardens ends the preview with no write. Only "Turn sync on" writes.

import { useEffect, useRef, useState } from 'react'
import { loadZenPage } from '@/lib/zen/zen-api'
import { useToastStore } from '@/stores/toast'
import {
  setZenGardenSync,
  undoZenGardenSync,
  useZenSyncPreviewStore,
  type ZenSyncParts,
} from '@/lib/zen/zen-sync'
import { useZenGardensStore } from '@/lib/zen/zen-gardens'

const barStyle: React.CSSProperties = {
  background: 'var(--zen-surface-raised)',
  color: 'var(--zen-text)',
  borderBottom: '1px solid var(--zen-border)',
}

const buttonStyle: React.CSSProperties = {
  minHeight: 24,
  padding: '2px 10px',
  border: '1px solid var(--zen-border)',
  borderRadius: 999,
  color: 'var(--zen-text)',
  background: 'var(--zen-surface)',
}

function partWords(part: ZenSyncParts): string {
  if (part === 'theme') return 'theme'
  if (part === 'page') return 'default'
  return 'default and theme'
}

export function ZenSyncPreviewBar({
  gardenId,
  gardenName,
  part,
}: {
  gardenId: string
  gardenName: string
  part: ZenSyncParts
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const end = useZenSyncPreviewStore((s) => s.end)
  const turnOn = (): void => {
    setBusy(true)
    setError(null)
    setZenGardenSync(gardenId, part, true)
      .then(() => {
        end()
        useToastStore.getState().addToast(`${gardenName} now syncs with K2’s default`, 'success', 8000, {
          label: 'Undo',
          onClick: () => void undoZenGardenSync(gardenId),
        })
      })
      .catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setBusy(false))
  }
  return (
    <div
      role="status"
      data-zen-sync-preview-bar=""
      data-zen-sync-preview-part={part}
      className="flex flex-shrink-0 flex-wrap items-center gap-3 px-4 py-2"
      style={barStyle}
    >
      <div className="min-w-0 flex-1" style={{ fontWeight: 600 }}>
        Previewing K2’s {partWords(part)} for {gardenName}
        {error && <span style={{ color: 'var(--zen-danger)', fontWeight: 400 }}> · {error}</span>}
      </div>
      <button type="button" className="no-drag cursor-pointer" style={buttonStyle} disabled={busy} onClick={turnOn} data-zen-sync-preview-on="">
        Turn sync on
      </button>
      <button type="button" className="no-drag cursor-pointer" style={buttonStyle} disabled={busy} onClick={end} data-zen-sync-preview-back="">
        Back to my copy
      </button>
    </div>
  )
}

/** ZenRoot's one hook (GS34): the preview bar for this window's Garden, or
 *  null. Re-reads the page when the preview starts or ends; ends the
 *  preview (no write) when the window switches Gardens or leaves Zen. */
export function useZenSyncPreviewBar(gardenId: string | null): React.JSX.Element | null {
  const preview = useZenSyncPreviewStore((s) => s.preview)
  const end = useZenSyncPreviewStore((s) => s.end)
  const gardens = useZenGardensStore((s) => s.gardens)
  const active = preview !== null && gardenId !== null && preview.gardenId === gardenId
  const key = active ? `${preview.gardenId}:${preview.part}` : ''
  useEffect(() => {
    if (preview && gardenId !== null && preview.gardenId !== gardenId) end()
  }, [preview, gardenId, end])
  useEffect(() => () => useZenSyncPreviewStore.getState().end(), [])
  // Re-read the page when the preview starts or ends (not on mount and
  // not on a Garden switch: ZenRoot loads those).
  const lastKey = useRef(key)
  useEffect(() => {
    if (lastKey.current === key) return
    lastKey.current = key
    if (gardenId !== null) void loadZenPage(gardenId)
  }, [key, gardenId])
  if (!active || !preview) return null
  const name = gardens.find((g) => g.id === preview.gardenId)?.name ?? 'this Garden'
  return <ZenSyncPreviewBar gardenId={preview.gardenId} gardenName={name} part={preview.part} />
}
