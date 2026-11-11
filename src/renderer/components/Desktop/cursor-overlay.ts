// The remote cursor over the Desktop picture (prd-remote-desktop-tab-v1
// §3.6, §8): capture runs with the cursor off, and deskd sends its shape
// once per shape (`cursor`) and its place up to 60 times a second
// (`cursor-pos`). Drawn outside React, the `Mobile/touch-dot.ts` way: a
// move sets the image's style directly, so the pane never re-renders for
// the cursor.
//
// React renders the `<img>` once and hands it over with `attach`; it gives
// it no `src` and no `style`, so a re-render never undoes what this sets.

import type { PictureLayout } from './desktop-input'

export interface CursorShape {
  url: string
  hotX: number
  hotY: number
}

export class CursorOverlay {
  private el: HTMLImageElement | null = null
  private readonly shapes = new Map<number, CursorShape>()
  private shape: number | null = null
  private at: { x: number; y: number; visible: boolean } | null = null
  private layout: PictureLayout | null = null
  /** Cursor image pixels per display point (the display's scale). */
  private imageScale = 1
  /** Moves applied (tests read it; a move is a style write, not a render). */
  moves = 0

  attach(el: HTMLImageElement | null): void {
    if (el === this.el) return
    this.el = el
    if (el) {
      el.style.position = 'absolute'
      el.style.left = '0'
      el.style.top = '0'
      el.style.pointerEvents = 'none'
      el.style.transformOrigin = '0 0'
      el.draggable = false
      this.apply()
    }
  }

  /** A shape from `cursor` (its PNG, base64). Kept by id; the next
   *  `cursor-pos` (or this, when it is the shape shown) draws it. */
  setShape(id: number, hotX: number, hotY: number, png: string): void {
    this.shapes.set(id, { url: `data:image/png;base64,${png}`, hotX, hotY })
    this.shape = id
    this.apply()
  }

  /** Where it is, in display points. */
  move(x: number, y: number, visible: boolean): void {
    this.at = { x, y, visible }
    this.apply()
  }

  /** The picture moved or was scaled (fit / 1:1, a resize, a new display). */
  setLayout(layout: PictureLayout | null, imageScale = 1): void {
    this.layout = layout
    this.imageScale = imageScale > 0 ? imageScale : 1
    this.apply()
  }

  /** The stream closed or the display changed: no cursor until told. */
  reset(): void {
    this.at = null
    this.apply()
  }

  dispose(): void {
    this.el = null
  }

  private apply(): void {
    const el = this.el
    if (!el) return
    const shape = this.shape === null ? undefined : this.shapes.get(this.shape)
    if (!shape || !this.at || !this.at.visible || !this.layout) {
      el.style.display = 'none'
      return
    }
    if (el.getAttribute('src') !== shape.url) el.setAttribute('src', shape.url)
    const k = this.layout.scale / this.imageScale
    const x = this.layout.ox + this.at.x * this.layout.scale
    const y = this.layout.oy + this.at.y * this.layout.scale
    el.style.display = ''
    el.style.transform = `translate3d(${x}px, ${y}px, 0) scale(${k}) translate(${-shape.hotX}px, ${-shape.hotY}px)`
    el.setAttribute('data-desktop-cursor', `${Math.round(this.at.x)},${Math.round(this.at.y)}`)
    this.moves++
  }
}
