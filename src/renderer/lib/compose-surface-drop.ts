// Image drops on the Thread column and the chat overlay attach to the
// Message-the-agent compose bar inside that surface. The bar itself still
// accepts every file type; this wider hit is images only, and it uses the
// same extensions the composer thumbnails (not PDF).

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

/** Compose bar inside the thread/chat surface under `el`, if any. */
export function surfaceComposeBar(el: HTMLElement | null): HTMLElement | null {
  if (!el?.closest) return null
  const surface = el.closest(COMPOSE_DROP_SURFACE_SELECTOR) as HTMLElement | null
  if (!surface?.querySelector) return null
  return surface.querySelector(COMPOSE_BAR_SELECTOR) as HTMLElement | null
}
