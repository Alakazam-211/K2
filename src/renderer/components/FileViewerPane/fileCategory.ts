// Pure file-classification helpers for FileViewerPane. Extracted into
// their own module (no React / Tauri imports) so they can be unit-tested
// without dragging in the lazy-loaded viewers or the tabs store's
// module-load side effects. FileViewerPane re-exports these.

export type FileCategory = 'markdown' | 'html' | 'image' | 'pdf' | 'docx' | 'text'
export type ViewMode = 'rendered' | 'raw'

export const MARKDOWN_EXTS = ['.md', '.markdown', '.mdx']
export const HTML_EXTS = ['.html', '.htm']
export const IMAGE_EXTS = ['.png', '.jpg', '.jpeg', '.gif', '.webp', '.svg', '.bmp', '.ico']
export const PDF_EXTS = ['.pdf']
export const DOCX_EXTS = ['.docx', '.doc']

export function getFileCategory(filePath: string): FileCategory {
  const ext = filePath.toLowerCase().replace(/^.*(\.[^.]+)$/, '$1')
  if (MARKDOWN_EXTS.includes(ext)) return 'markdown'
  if (HTML_EXTS.includes(ext)) return 'html'
  if (IMAGE_EXTS.includes(ext)) return 'image'
  if (PDF_EXTS.includes(ext)) return 'pdf'
  if (DOCX_EXTS.includes(ext)) return 'docx'
  return 'text'
}

export function getDefaultViewMode(category: FileCategory): ViewMode {
  // HTML prefers the rendered view (like markdown) — most HTML files
  // the user opens here are dashboards/reports they want to *see*, not
  // hand-edit. The Edit toggle is one click away in the top bar.
  if (category === 'markdown' || category === 'html' || category === 'image') return 'rendered'
  return 'raw'
}
