// The minimap, copied from Board's (`board-file-1/ui.js`, "zoom bar +
// minimap": drawMinimap, minimapGo). The canvas draws buckets and cards in
// K2's tokens (read from the page, so both Styles and schemes follow), and a
// press or drag on it moves the camera there.

import { useEffect, useRef } from 'react'
import { BoardGeom, type Box } from './board-geom'
import type { Camera } from './org-camera'
import type { OrgLayout } from './org-layout'

const W = 220
const H = 140

interface MapView {
  k: number
  ox: number
  oy: number
}

export function OrgMinimap({
  layout,
  cam,
  viewport,
  onCamera,
}: {
  layout: OrgLayout
  cam: Camera
  /** The viewport's size on screen. */
  viewport: { w: number; h: number }
  onCamera: (cam: Camera) => void
}): React.JSX.Element {
  const ref = useRef<HTMLCanvasElement>(null)
  const view = useRef<MapView | null>(null)

  useEffect(() => {
    const el = ref.current
    const ctx = el?.getContext('2d')
    if (!el || !ctx) return
    const seen: Box = { x: -cam.x / cam.s, y: -cam.y / cam.s, w: viewport.w / cam.s, h: viewport.h / cam.s }
    const all = BoardGeom.bounds([layout.bounds, seen])!
    const pad = 40 / Math.max(0.2, Math.min(1, cam.s))
    const box = { x: all.x - pad, y: all.y - pad, w: all.w + pad * 2, h: all.h + pad * 2 }
    const k = Math.min(W / box.w, H / box.h)
    const ox = (W - box.w * k) / 2 - box.x * k
    const oy = (H - box.h * k) / 2 - box.y * k
    view.current = { k, ox, oy }
    const css = getComputedStyle(document.documentElement)
    const token = (name: string): string => css.getPropertyValue(name).trim()
    ctx.clearRect(0, 0, W, H)
    for (const b of layout.orgs.values()) {
      ctx.fillStyle = token('--color-bg-elevated')
      ctx.fillRect(b.x * k + ox, b.y * k + oy, Math.max(1.5, b.w * k), Math.max(1.5, b.h * k))
      ctx.strokeStyle = token('--color-border-strong')
      ctx.lineWidth = 2
      ctx.strokeRect(b.x * k + ox, b.y * k + oy, Math.max(1.5, b.w * k), Math.max(1.5, b.h * k))
    }
    ctx.fillStyle = token('--color-text-muted')
    for (const a of layout.agents.values()) ctx.fillRect(a.x * k + ox, a.y * k + oy, Math.max(1.5, a.w * k), Math.max(1.5, a.h * k))
    const accent = token('--color-accent')
    ctx.strokeStyle = accent
    ctx.lineWidth = 3
    ctx.fillStyle = accent
    ctx.globalAlpha = 0.08
    ctx.fillRect(seen.x * k + ox, seen.y * k + oy, seen.w * k, seen.h * k)
    ctx.globalAlpha = 1
    ctx.strokeRect(seen.x * k + ox, seen.y * k + oy, seen.w * k, seen.h * k)
  }, [layout, cam, viewport.w, viewport.h])

  const go = (e: React.PointerEvent<HTMLCanvasElement>): void => {
    const m = view.current
    const el = ref.current
    if (!m || !el) return
    const r = el.getBoundingClientRect()
    const sx = (e.clientX - r.left) * (W / (r.width || W))
    const sy = (e.clientY - r.top) * (H / (r.height || H))
    const wx = (sx - m.ox) / m.k
    const wy = (sy - m.oy) / m.k
    onCamera({ ...cam, x: viewport.w / 2 - wx * cam.s, y: viewport.h / 2 - wy * cam.s })
  }

  return (
    <canvas
      ref={ref}
      width={W}
      height={H}
      data-testid="org-chart-minimap"
      aria-label="Minimap: press to move the view there"
      className="block cursor-pointer"
      style={{ width: W, height: H }}
      onPointerDown={(e) => {
        e.stopPropagation()
        go(e)
        try {
          e.currentTarget.setPointerCapture(e.pointerId)
        } catch {
          // No capture (a synthetic event): the press alone still moves the view.
        }
      }}
      onPointerMove={(e) => {
        if (e.buttons & 1) go(e)
      }}
    />
  )
}
