// A live deck's slides as the daemon draws them (V3), in the file's tab
// while the Slides core is on. The daemon owns everything: which slides
// exist, their render hashes, drawing only the slides that changed, the
// cache. This pane asks for the list every few seconds while its tab is on
// screen, fetches each thumb when its hash moves, and the selected slide at
// full width. Images are object URLs, revoked when replaced or unmounted.
//
// A first, plain view: K2 Design gives it its look later (filmstrip, stage,
// overlays, activity line).

import React, { useCallback, useEffect, useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { errorMessage, httpCodeOf } from '@/lib/daemon-http-error'
import { fetchDeckListing, fetchPreviewImage, type DeckListing, type PreviewWidth } from '@/lib/docs-preview'

export const POLL_MS = 3_000

export interface DeckPreviewPaneProps {
  scope: ServerScope
  /** The deck, absolute (the daemon finds its workspace). */
  filePath: string
}

interface Shown {
  hash: string
  url: string
  stale: boolean
}

const muted = 'text-[var(--color-text-muted)]'

function revokeAll(m: Map<string, Shown>): void {
  for (const s of m.values()) URL.revokeObjectURL(s.url)
}

export function DeckPreviewPane({ scope, filePath }: DeckPreviewPaneProps): React.JSX.Element {
  const [listing, setListing] = useState<DeckListing | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<string | null>(null)
  const [thumbs, setThumbs] = useState<Map<string, Shown>>(new Map())
  const [full, setFull] = useState<{ key: string; shown: Shown | null; note: string | null } | null>(null)
  const thumbsRef = useRef(thumbs)
  thumbsRef.current = thumbs
  const fullRef = useRef(full)
  fullRef.current = full

  const fetchImage = useCallback(
    async (key: string, width: PreviewWidth): Promise<{ shown: Shown | null; note: string | null }> => {
      const img = await fetchPreviewImage(scope, filePath, key, width)
      if (img.state === 'failed') return { shown: null, note: `couldn't draw this slide (${img.failedCode ?? 'renderer_error'})` }
      if (!img.bytes) return { shown: null, note: 'drawing…' }
      const url = URL.createObjectURL(new Blob([img.bytes as BlobPart], { type: img.mime ?? 'image/webp' }))
      return { shown: { hash: img.renderHash, url, stale: img.state === 'stale' }, note: img.state === 'stale' ? 'updating…' : null }
    },
    [scope, filePath],
  )

  const poll = useCallback(
    async (isCurrent: () => boolean) => {
      let l: DeckListing
      try {
        l = await fetchDeckListing(scope, filePath, 'thumb')
      } catch (e) {
        if (!isCurrent()) return
        setError(httpCodeOf(e) === 'no_preview' ? 'No previews for this file here.' : errorMessage(e))
        return
      }
      if (!isCurrent()) return
      setError(null)
      setListing(l)
      setSelected((s) => (s && l.slides.some((x) => x.key === s) ? s : (l.slides[0]?.key ?? null)))
      // Thumbs whose hash moved (or that aren't shown yet) and are drawn.
      const next = new Map(thumbsRef.current)
      let changed = false
      for (const slide of l.slides) {
        const have = next.get(slide.key)
        if (slide.thumb !== 'ready' || (have && have.hash === slide.renderHash)) continue
        try {
          const got = await fetchImage(slide.key, 'thumb')
          if (!isCurrent()) {
            if (got.shown) URL.revokeObjectURL(got.shown.url)
            return
          }
          if (got.shown) {
            if (have) URL.revokeObjectURL(have.url)
            next.set(slide.key, got.shown)
            changed = true
          }
        } catch {
          // Asked again on the next poll.
        }
      }
      for (const [key, s] of next) {
        if (!l.slides.some((x) => x.key === key)) {
          URL.revokeObjectURL(s.url)
          next.delete(key)
          changed = true
        }
      }
      if (changed) setThumbs(next)
    },
    [scope, filePath, fetchImage],
  )

  useEffect(() => {
    let current = true
    const isCurrent = () => current
    void poll(isCurrent)
    const t = setInterval(() => {
      if (typeof document === 'undefined' || document.visibilityState !== 'hidden') void poll(isCurrent)
    }, POLL_MS)
    return () => {
      current = false
      clearInterval(t)
    }
  }, [poll])

  useEffect(
    () => () => {
      revokeAll(thumbsRef.current)
      const f = fullRef.current?.shown
      if (f) URL.revokeObjectURL(f.url)
    },
    [],
  )

  // The selected slide at full width, again whenever its hash moves.
  const selectedSlide = listing?.slides.find((s) => s.key === selected) ?? null
  const wantHash = selectedSlide?.renderHash ?? null
  useEffect(() => {
    if (!selected || !wantHash) return
    const have = fullRef.current
    if (have && have.key === selected && have.shown && have.shown.hash === wantHash && !have.shown.stale) return
    let current = true
    void (async () => {
      try {
        const got = await fetchImage(selected, 'full')
        if (!current) {
          if (got.shown) URL.revokeObjectURL(got.shown.url)
          return
        }
        setFull((prev) => {
          if (prev?.shown && prev.shown.url !== got.shown?.url) URL.revokeObjectURL(prev.shown.url)
          return { key: selected, shown: got.shown, note: got.note }
        })
      } catch (e) {
        if (current) setFull({ key: selected, shown: null, note: errorMessage(e) })
      }
    })()
    return () => {
      current = false
    }
    // `listing` moves on every poll: re-ask while a full image is still drawing.
  }, [selected, wantHash, fetchImage, listing])

  if (error && !listing) {
    return (
      <div className="flex h-full items-center justify-center p-4" data-testid="deck-preview-error">
        <p className={`text-xs ${muted}`}>{error}</p>
      </div>
    )
  }
  if (!listing) {
    return <div className={`p-4 text-xs ${muted}`}>Loading slides…</div>
  }
  const aspect = listing.page && listing.page.w > 0 ? listing.page.h / listing.page.w : 9 / 16
  const fullShown = full && full.key === selected ? full : null
  return (
    <div className="flex h-full overflow-hidden" data-testid="deck-preview">
      <ol className="w-44 flex-shrink-0 overflow-y-auto border-r border-[var(--color-border)] p-2" aria-label="Slides">
        {listing.slides.map((s) => {
          const t = thumbs.get(s.key)
          const label = `Slide ${s.position}${s.title ? `: ${s.title}` : ''}`
          return (
            <li key={s.key} className="mb-2">
              <button
                type="button"
                onClick={() => setSelected(s.key)}
                aria-current={s.key === selected ? 'true' : undefined}
                className={`block w-full rounded-[var(--radius-box)] border p-1 text-left ${
                  s.key === selected ? 'border-[var(--color-accent)]' : 'border-[var(--color-border)]'
                } ${s.hidden ? 'opacity-50' : ''}`}
              >
                <div className="relative w-full bg-[var(--color-bg-stripe)]" style={{ paddingTop: `${aspect * 100}%` }}>
                  {t ? (
                    <img src={t.url} alt={label} className="absolute inset-0 h-full w-full object-contain" />
                  ) : (
                    <span className={`absolute inset-0 flex items-center justify-center text-[10px] ${muted}`}>
                      {s.thumb === 'failed' ? "can't draw" : 'drawing…'}
                    </span>
                  )}
                </div>
                <span className={`mt-1 block truncate text-[10px] ${muted}`}>{label}</span>
              </button>
            </li>
          )
        })}
      </ol>
      <div className="flex flex-1 flex-col items-center justify-center overflow-auto p-4">
        {fullShown?.shown ? (
          <img
            src={fullShown.shown.url}
            alt={selectedSlide ? `Slide ${selectedSlide.position}${selectedSlide.title ? `: ${selectedSlide.title}` : ''}` : 'Slide'}
            className="max-h-full max-w-full object-contain shadow"
            data-testid="deck-preview-stage"
          />
        ) : null}
        {fullShown?.note || !fullShown?.shown ? (
          <p className={`mt-2 text-xs ${muted}`} data-testid="deck-preview-note">
            {fullShown?.note ?? 'drawing…'}
          </p>
        ) : null}
      </div>
    </div>
  )
}
