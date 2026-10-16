// Can this window run the Wiki 3D view well? (prd-wiki-3d-view-v1 §3.8, §6.7)
//
// MAIN BUNDLE: this module must never import `three` (W4) — the CAD
// viewer's `hasWebGL2` lives next to a static `three` import, so the Wiki
// page can't use it without pulling three into the main bundle.

export type Wiki3dFallbackReason =
  | 'noWebGL2'
  | 'software'
  | 'tooMany'
  | 'slow'
  | 'loadFailed'
  | 'crashed'

export type Wiki3dProbe = {
  /** Why 3D can't run here before even loading the chunk; null = go. */
  blockReason: 'noWebGL2' | 'software' | null
  /** Unmasked GPU name when the browser exposes it. */
  renderer: string | null
}

/** Above this many visible nodes the 3D view falls back to 2D (F3). */
export const MAX_3D_NODES = 3000

/** Median fps under this drops glow and caps labels (degrade step). */
export const FPS_DEGRADE_BELOW = 30
/** Median fps under this falls back to 2D (F4). */
export const FPS_FALLBACK_BELOW = 20
/** How long the fps watchdog samples, after the first engine start. */
export const FPS_SAMPLE_MS = 3000

const SOFTWARE_GPU = /llvmpipe|swiftshader|softpipe|software/i

/** The 3D button's title when this window has no WebGL 2. */
export const NO_WEBGL2_TITLE = "3D needs WebGL 2, which this window doesn't have."

export function fallbackToastCopy(reason: Wiki3dFallbackReason, nodeCount?: number): string {
  switch (reason) {
    case 'noWebGL2':
      return "3D needs WebGL 2, which this window doesn't have. Showing 2D."
    case 'software':
      return "3D needs graphics acceleration, which this machine isn't using. Showing 2D."
    case 'tooMany':
      return `Too many notes for 3D (${(nodeCount ?? 0).toLocaleString('en-US')}). Showing 2D.`
    case 'slow':
      return '3D was running slowly here. Showing 2D.'
    case 'loadFailed':
      return "Couldn't load the 3D view. Showing 2D."
    case 'crashed':
      return '3D view stopped. Showing 2D.'
  }
}

type GL = WebGL2RenderingContext

function loseContext(gl: GL | null): void {
  try {
    gl?.getExtension('WEBGL_lose_context')?.loseContext()
  } catch {
    /* already gone */
  }
}

function unmaskedRenderer(gl: GL): string | null {
  try {
    const ext = gl.getExtension('WEBGL_debug_renderer_info') as
      | { UNMASKED_RENDERER_WEBGL: number }
      | null
    if (!ext) return null
    const name = gl.getParameter(ext.UNMASKED_RENDERER_WEBGL)
    return typeof name === 'string' && name ? name : null
  } catch {
    return null
  }
}

/**
 * Probe a throwaway canvas: WebGL 2 with `failIfMajorPerformanceCaveat`;
 * if that fails but plain WebGL 2 works, it's software rendering. A GPU
 * name like llvmpipe / SwiftShader is software too. Every probe context is
 * released at once (WebKit caps live contexts). WebKit may not honour the
 * caveat flag or expose the GPU name; the fps watchdog is the backstop.
 */
export function probeWiki3d(doc: Document | undefined = typeof document === 'undefined' ? undefined : document): Wiki3dProbe {
  if (!doc) return { blockReason: 'noWebGL2', renderer: null }
  let strict: GL | null = null
  try {
    strict = doc.createElement('canvas').getContext('webgl2', { failIfMajorPerformanceCaveat: true }) as GL | null
  } catch {
    strict = null
  }
  if (strict) {
    const name = unmaskedRenderer(strict)
    loseContext(strict)
    if (name && SOFTWARE_GPU.test(name)) return { blockReason: 'software', renderer: name }
    return { blockReason: null, renderer: name }
  }
  let plain: GL | null = null
  try {
    plain = doc.createElement('canvas').getContext('webgl2') as GL | null
  } catch {
    plain = null
  }
  if (!plain) return { blockReason: 'noWebGL2', renderer: null }
  const name = unmaskedRenderer(plain)
  loseContext(plain)
  return { blockReason: 'software', renderer: name }
}

export type FpsVerdict = 'ok' | 'degrade' | 'fallback'

/** Median of per-frame fps samples → ok / degrade (< 30) / fallback (< 20). */
export function classifyFps(samples: readonly number[]): FpsVerdict {
  if (samples.length === 0) return 'ok'
  const sorted = [...samples].sort((a, b) => a - b)
  const mid = Math.floor(sorted.length / 2)
  const median = sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2
  if (median < FPS_FALLBACK_BELOW) return 'fallback'
  if (median < FPS_DEGRADE_BELOW) return 'degrade'
  return 'ok'
}

/** True when the node count is over the 3D cap (F3). */
export function tooManyFor3d(nodeCount: number): boolean {
  return nodeCount > MAX_3D_NODES
}
