// Pure file-classification helpers for FileViewerPane. Extracted into
// their own module (no React / Tauri imports) so they can be unit-tested
// without dragging in the lazy-loaded viewers or the tabs store's
// module-load side effects. FileViewerPane re-exports these.

export type FileCategory =
  | 'markdown'
  | 'html'
  | 'image'
  | 'pdf'
  | 'docx'
  | 'text'
  | 'csv'
  | 'audio'
  | 'video'
  | 'zip'
  | 'diagram'
  | 'diagramSource'
  | 'binary'

export type ViewMode = 'rendered' | 'raw'

export const MARKDOWN_EXTS = ['.md', '.markdown', '.mdx']
export const HTML_EXTS = ['.html', '.htm']
export const IMAGE_EXTS = [
  '.png',
  '.jpg',
  '.jpeg',
  '.gif',
  '.webp',
  '.svg',
  '.bmp',
  '.ico',
  '.heic',
  '.avif',
  '.tif',
  '.tiff',
]
export const PDF_EXTS = ['.pdf']
export const DOCX_EXTS = ['.docx', '.doc']
export const CSV_EXTS = ['.csv', '.tsv']
export const AUDIO_EXTS = ['.mp3', '.wav', '.ogg', '.flac', '.m4a', '.aac']
export const VIDEO_EXTS = ['.mp4', '.webm', '.mov', '.mkv']
export const ZIP_EXTS = ['.zip']
/** Mermaid diagrams — dual mode with rendered preview. */
export const DIAGRAM_EXTS = ['.mermaid', '.mmd']
/**
 * Diagram *source* files: preview is a structured empty/source state in v1
 * (no full diagrams.net / PlantUML server embed); raw edit still available.
 */
export const DIAGRAM_SOURCE_EXTS = [
  '.drawio',
  '.dio',
  '.puml',
  '.plantuml',
  '.dot',
  '.gv',
]
/** Known binary extensions — never dump into CodeMirror. */
export const BINARY_EXTS = [
  '.wasm',
  '.bin',
  '.exe',
  '.dll',
  '.so',
  '.dylib',
  '.class',
  '.o',
  '.a',
  '.lib',
  '.obj',
  '.woff',
  '.woff2',
  '.ttf',
  '.otf',
  '.eot',
  '.pyc',
  '.pyo',
  '.pdb',
  '.dat',
]

export function getFileCategory(filePath: string): FileCategory {
  const ext = filePath.toLowerCase().replace(/^.*(\.[^.]+)$/, '$1')
  if (MARKDOWN_EXTS.includes(ext)) return 'markdown'
  if (HTML_EXTS.includes(ext)) return 'html'
  if (IMAGE_EXTS.includes(ext)) return 'image'
  if (PDF_EXTS.includes(ext)) return 'pdf'
  if (DOCX_EXTS.includes(ext)) return 'docx'
  if (CSV_EXTS.includes(ext)) return 'csv'
  if (AUDIO_EXTS.includes(ext)) return 'audio'
  if (VIDEO_EXTS.includes(ext)) return 'video'
  if (ZIP_EXTS.includes(ext)) return 'zip'
  if (DIAGRAM_EXTS.includes(ext)) return 'diagram'
  if (DIAGRAM_SOURCE_EXTS.includes(ext)) return 'diagramSource'
  if (BINARY_EXTS.includes(ext)) return 'binary'
  return 'text'
}

/**
 * Categories that never load as text (binary bytes, dedicated viewers, or
 * empty-state-only panes). Used to skip `fs/read-file` and text polling.
 */
export function isBinaryOrMediaCategory(category: FileCategory): boolean {
  return (
    category === 'image' ||
    category === 'pdf' ||
    category === 'docx' ||
    category === 'audio' ||
    category === 'video' ||
    category === 'zip' ||
    category === 'binary'
  )
}

export function getDefaultViewMode(category: FileCategory): ViewMode {
  // Dual-mode previews default to rendered (markdown-style).
  if (
    category === 'markdown' ||
    category === 'html' ||
    category === 'image' ||
    category === 'csv' ||
    category === 'diagram' ||
    category === 'diagramSource'
  ) {
    return 'rendered'
  }
  // Media + zip + binary: rendered-only empty-ish / dedicated viewer; no raw edit.
  if (
    category === 'audio' ||
    category === 'video' ||
    category === 'zip' ||
    category === 'binary' ||
    category === 'pdf' ||
    category === 'docx'
  ) {
    return 'rendered'
  }
  // text/code
  return 'raw'
}
