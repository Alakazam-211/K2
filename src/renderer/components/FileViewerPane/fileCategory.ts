// Pure file-classification helpers for FileViewerPane. Extracted into
// their own module (no React / Tauri imports) so they can be unit-tested
// without dragging in the lazy-loaded viewers or the tabs store's
// module-load side effects. FileViewerPane re-exports these.
//
// Phase A: Rust daemon owns bytes/list/extract; Tauri webview presents
// (img/audio/video/table/mermaid). No convertFileSrc for host files.

export type FileCategory =
  | 'markdown'
  | 'html'
  | 'image'
  | 'pdf'
  | 'docx'
  | 'csv'
  | 'audio'
  | 'video'
  | 'zip'
  | 'mermaid'
  | 'diagram'
  | 'binary'
  | 'text'

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
export const MERMAID_EXTS = ['.mermaid', '.mmd']
/** Diagram sources without a full client renderer in Phase A. */
export const DIAGRAM_EXTS = ['.drawio', '.dio', '.puml', '.plantuml', '.dot', '.gv']
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

export function getFileExtension(filePath: string): string {
  const lower = filePath.toLowerCase()
  const match = lower.match(/(\.[^.]+)$/)
  return match ? match[1] : ''
}

export function getFileCategory(filePath: string): FileCategory {
  const ext = getFileExtension(filePath)
  if (MARKDOWN_EXTS.includes(ext)) return 'markdown'
  if (HTML_EXTS.includes(ext)) return 'html'
  if (IMAGE_EXTS.includes(ext)) return 'image'
  if (PDF_EXTS.includes(ext)) return 'pdf'
  if (DOCX_EXTS.includes(ext)) return 'docx'
  if (CSV_EXTS.includes(ext)) return 'csv'
  if (AUDIO_EXTS.includes(ext)) return 'audio'
  if (VIDEO_EXTS.includes(ext)) return 'video'
  if (ZIP_EXTS.includes(ext)) return 'zip'
  if (MERMAID_EXTS.includes(ext)) return 'mermaid'
  if (DIAGRAM_EXTS.includes(ext)) return 'diagram'
  if (BINARY_EXTS.includes(ext)) return 'binary'
  return 'text'
}

export function getDefaultViewMode(category: FileCategory): ViewMode {
  if (
    category === 'markdown' ||
    category === 'html' ||
    category === 'image' ||
    category === 'csv' ||
    category === 'audio' ||
    category === 'video' ||
    category === 'zip' ||
    category === 'mermaid' ||
    category === 'diagram' ||
    category === 'binary'
  ) {
    return 'rendered'
  }
  return 'raw'
}

/** Categories that load bytes (not text) and skip the text poll/save path. */
export function isBinaryPreviewCategory(category: FileCategory): boolean {
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

/** Categories that show the Preview | Edit toggle. */
export function supportsViewToggle(category: FileCategory): boolean {
  return (
    category === 'markdown' ||
    category === 'html' ||
    category === 'image' ||
    category === 'csv' ||
    category === 'mermaid' ||
    category === 'diagram'
  )
}

const MEDIA_MIME_BY_EXT: Record<string, string> = {
  mp3: 'audio/mpeg',
  wav: 'audio/wav',
  ogg: 'audio/ogg',
  flac: 'audio/flac',
  m4a: 'audio/mp4',
  aac: 'audio/aac',
  mp4: 'video/mp4',
  webm: 'video/webm',
  mov: 'video/quicktime',
  mkv: 'video/x-matroska',
}

/** MIME for HTML5 audio/video elements from path extension. */
export function mediaMimeFromPath(path: string): string {
  const ext = getFileExtension(path).replace(/^\./, '')
  return MEDIA_MIME_BY_EXT[ext] ?? 'application/octet-stream'
}
