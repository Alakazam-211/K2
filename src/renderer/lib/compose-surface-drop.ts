// A file dropped anywhere on a Thread pane (its messages, header, empty
// state or message box) attaches to the message box inside that pane, as a
// drop on the box itself does. The pane is `[data-compose-drop-surface]`;
// its box is the `[data-compose-bar]` inside it. A box marked
// `data-compose-disabled` (or no box at all: a view-only room) takes no
// drop, so the pane doesn't either.
//
// OS drops (external-drop-router) take any file on the pane. The Files
// drawer's in-window drag (file-drag.ts) still attaches images only on the
// pane, and any file on the box itself: the same extensions the composer
// thumbnails (not PDF).

export const COMPOSE_BAR_SELECTOR = '[data-compose-bar]'
export const COMPOSE_DROP_SURFACE_SELECTOR = '[data-compose-drop-surface]'

/** Same stills `isComposePreviewImagePath` thumbnails. Not PDF. */
const COMPOSE_SURFACE_IMAGE_RE =
  /\.(png|jpe?g|gif|webp|bmp|heic|heif|svg)$/i

const COMPOSE_SURFACE_IMAGE_TYPES = new Set([
  'image/png',
  'image/jpeg',
  'image/gif',
  'image/webp',
  'image/bmp',
  'image/heic',
  'image/heif',
  'image/svg+xml',
])

export function isComposeSurfaceImagePath(path: string): boolean {
  return COMPOSE_SURFACE_IMAGE_RE.test(path.trim())
}

/** Every path is a composer image. Empty and mixed drops are not. */
export function pathsAreComposeSurfaceImages(paths: string[]): boolean {
  return paths.length > 0 && paths.every((p) => isComposeSurfaceImagePath(p))
}

/**
 * Composer image file. A filename with a non-image extension loses even
 * when the MIME says image/* — the draft chips key off the path extension.
 * Extension-less names (clipboard "image") accept a known image MIME.
 */
export function isComposeSurfaceImageFile(file: { name?: string; type?: string }): boolean {
  const name = (file.name ?? '').trim()
  if (name && isComposeSurfaceImagePath(name)) return true
  if (name && /\.[A-Za-z0-9]+$/.test(name)) return false
  const type = (file.type ?? '').split(';')[0].trim().toLowerCase()
  return COMPOSE_SURFACE_IMAGE_TYPES.has(type)
}

export function filesAreComposeSurfaceImages(
  files: Array<{ name?: string; type?: string }>,
): boolean {
  return files.length > 0 && files.every((f) => isComposeSurfaceImageFile(f))
}

/** Compose bar inside the thread/chat surface under `el`, if any. A
 *  disabled box (`data-compose-disabled`) takes no drop: null. */
export function surfaceComposeBar(el: HTMLElement | null): HTMLElement | null {
  if (!el?.closest) return null
  const surface = el.closest(COMPOSE_DROP_SURFACE_SELECTOR) as HTMLElement | null
  if (!surface?.querySelector) return null
  const bar = surface.querySelector(COMPOSE_BAR_SELECTOR) as HTMLElement | null
  if (!bar || bar.dataset?.composeDisabled !== undefined) return null
  return bar
}

/** The Thread pane under `el` that takes a file drop (it holds a box that
 *  takes files), or null. */
export function composeDropSurfaceAt(el: HTMLElement | null): HTMLElement | null {
  if (!surfaceComposeBar(el)) return null
  return el?.closest?.(COMPOSE_DROP_SURFACE_SELECTOR) as HTMLElement | null
}

/** Dispatched on a Thread pane while an OS file drag (Tauri's own drag
 *  events, which the page never sees as HTML5 drags) is over it, and again
 *  when it leaves: `detail.over`. The pane draws its drop state. */
export const COMPOSE_SURFACE_DROP_OVER_EVENT = 'k2:compose-surface-drop-over'

export interface ComposeSurfaceDropOverDetail {
  over: boolean
}

let markedSurface: HTMLElement | null = null

/** Mark the one Thread pane an OS file drag is over (null: none). */
export function markComposeSurfaceDropOver(surface: HTMLElement | null): void {
  if (markedSurface === surface) return
  const prev = markedSurface
  markedSurface = surface
  prev?.dispatchEvent(
    new CustomEvent<ComposeSurfaceDropOverDetail>(COMPOSE_SURFACE_DROP_OVER_EVENT, { detail: { over: false } }),
  )
  surface?.dispatchEvent(
    new CustomEvent<ComposeSurfaceDropOverDetail>(COMPOSE_SURFACE_DROP_OVER_EVENT, { detail: { over: true } }),
  )
}
