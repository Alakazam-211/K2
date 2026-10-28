// Garden widget frames that take OS file drops on the desktop
// (prd-garden-board-needs-v1 GP4, vs-live V5). Tauri owns every Finder drop
// (`tauri://drag-drop`), so a sealed frame never sees one: the external drop
// router asks here whether the point is over a frame whose widget listens
// (`k2.onDrop`), and hands that frame the paths. A frame that doesn't listen
// isn't here, so its drops fall through as before. While a drag is over a
// listening frame, K2 draws a ring on it (the frame can't see a Tauri drag).

/** Where in the frame (its CSS pixels) the files were dropped. */
export type ZenDropHandler = (paths: string[], at: { x: number; y: number }) => void

const targets = new Map<HTMLIFrameElement, ZenDropHandler>()

/** Take drops on `frame` until the returned function runs. */
export function registerZenDropTarget(frame: HTMLIFrameElement, handler: ZenDropHandler): () => void {
  targets.set(frame, handler)
  return () => {
    if (targets.get(frame) === handler) targets.delete(frame)
    if (ringed === frame) markZenDropOver(null)
  }
}

/** The listening frame `el` is (the element under a drop point), if any. */
export function zenDropTargetAt(el: Element | null): { frame: HTMLIFrameElement; handler: ZenDropHandler } | null {
  if (!el || el.tagName !== 'IFRAME') return null
  const frame = el as HTMLIFrameElement
  const handler = targets.get(frame)
  return handler ? { frame, handler } : null
}

/** A window point → the frame's own coordinates. */
export function zenFramePoint(frame: HTMLIFrameElement, p: { x: number; y: number }): { x: number; y: number } {
  const r = frame.getBoundingClientRect()
  return { x: Math.round(p.x - r.left), y: Math.round(p.y - r.top) }
}

let ringed: HTMLIFrameElement | null = null

/** The drop ring on one frame (null clears it). */
export function markZenDropOver(frame: HTMLIFrameElement | null): void {
  if (ringed === frame) return
  if (ringed) {
    ringed.removeAttribute('data-zen-drop-over')
    ringed.style.outline = ''
    ringed.style.outlineOffset = ''
  }
  ringed = frame
  if (frame) {
    frame.setAttribute('data-zen-drop-over', '')
    frame.style.outline = '2px dashed var(--zen-accent, #3b82f6)'
    frame.style.outlineOffset = '-2px'
  }
}

/** Tests: forget every target. */
export function resetZenDropTargets(): void {
  markZenDropOver(null)
  targets.clear()
}
