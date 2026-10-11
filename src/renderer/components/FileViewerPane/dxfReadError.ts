// Pure (no React, no dxf-parser) so FileViewerPane can import it eagerly
// without pulling the lazy DXF chunk into the start-up bundle.

/**
 * `fs/read-file` refusals the DXF preview gets around by reading bytes:
 * its 10 MB text cap ("File too large (>10MB)") and its binary check
 * ("Cannot read binary file": a NUL byte, i.e. binary DXF, or text that
 * is not UTF-8, i.e. an older ANSI-codepage DXF).
 */
export function dxfReadsAsBytes(error: string): boolean {
  return /binary|too large/i.test(error)
}

/** Message for any other DXF read failure (not found, permission, …). */
export function dxfReadErrorNote(error: string): { title: string; detail: string } {
  if (/not found/i.test(error)) return { title: 'File not found', detail: error }
  return { title: 'Could not open the DXF', detail: error }
}
