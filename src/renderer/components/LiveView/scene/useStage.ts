// The stage's hooks: the dark palette it re-scopes (from the look on
// <html>, following a change of look live), with the viewer's accent
// re-derived for that palette so a custom accent still reads on the stage;
// the pane's size (for the letterbox); and the still for a request (the
// last one stays up until the next lands).

import { useEffect, useMemo, useRef, useState } from 'react'
import { accentVarsFor } from '@/lib/style-accent'
import { stagePaletteMeta, stageScope, type StageScope } from '@/lib/live-view/scene/stage'
import type { PictureSource, Still, StillRequest } from '@/lib/live-view/scene/picture-source'
import { useStyleStore } from '@/stores/style'

const read = (): [string | null, string | null, string | null] => {
  if (typeof document === 'undefined') return [null, null, null]
  const h = document.documentElement
  return [h.getAttribute('data-style'), h.getAttribute('data-palette'), h.getAttribute('data-scheme')]
}

/** The look on <html>, live. */
function useLook(): [string | null, string | null, string | null] {
  const [look, setLook] = useState(read)
  useEffect(() => {
    if (typeof MutationObserver === 'undefined') return
    const mo = new MutationObserver(() => setLook((cur) => {
      const next = read()
      return next.every((v, i) => v === cur[i]) ? cur : next
    }))
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-style', 'data-palette', 'data-scheme'] })
    return () => mo.disconnect()
  }, [])
  return look
}

/** The stage's attributes and the accent's properties for its palette. */
export function useStageScope(): { scope: StageScope | null; style: React.CSSProperties | undefined } {
  const [style, palette, scheme] = useLook()
  const accent = useStyleStore((s) => s.accent)
  return useMemo(() => {
    const scope = stageScope(style, palette, scheme)
    if (!scope || scope['data-palette'] === palette) return { scope, style: undefined }
    const meta = stagePaletteMeta(scope)
    const vars = meta ? accentVarsFor(accent, meta, 'dark') : {}
    return { scope, style: Object.keys(vars).length ? (vars as React.CSSProperties) : undefined }
  }, [style, palette, scheme, accent])
}

/** An element's size, measured (or given, in tests and shots that fix it). */
export function usePaneSize(ref: React.RefObject<HTMLElement | null>, given?: { width: number; height: number }): { width: number; height: number } | null {
  const [size, setSize] = useState<{ width: number; height: number } | null>(null)
  useEffect(() => {
    const el = ref.current
    if (given || !el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(([e]) => {
      const w = Math.round(e.contentRect.width)
      const h = Math.round(e.contentRect.height)
      setSize((s) => (s && s.width === w && s.height === h ? s : w > 0 && h > 0 ? { width: w, height: h } : null))
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [ref, given])
  return given ?? size
}

const reqKey = (r: StillRequest | null): string => (r ? `${r.rev}|${r.view}|${r.shape}|${r.width}|${r.ids ? 1 : 0}` : '')

/** The still for a request; the one on screen stays until the next lands. Null when there's no request. */
export function useStill(source: PictureSource | null, req: StillRequest | null): Still | null {
  const [still, setStill] = useState<Still | null>(null)
  const key = reqKey(req)
  const latest = useRef(req)
  latest.current = req
  useEffect(() => {
    const r = latest.current
    if (!source || !r) {
      setStill(null)
      return
    }
    const ac = new AbortController()
    void source.still(r, ac.signal).then((st) => {
      if (!ac.signal.aborted) setStill(st)
    })
    return () => ac.abort()
  }, [source, key])
  return req ? still : null
}
