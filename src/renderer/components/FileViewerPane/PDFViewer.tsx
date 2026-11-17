// A PDF file's tab: the document (usePdfDocument, read again on a new
// version) in the PDF tab (components/Pdf/PdfTab): layers, annotations,
// text selection and Find. While it loads or fails, the header stays.
// The Map polyfill goes first: pdf.js 5 needs it on older WebKit.

import '@/lib/pdf/install-map-polyfill'
import { LiveHeader } from '@/components/LiveView/shell/LiveHeader'
import { fileNameOf } from '@/components/Editor/EditorFrame'
import { SetiFileIcon } from '@/lib/seti-file-icons/SetiFileIcon'
import { PdfTab } from '@/components/Pdf/PdfTab'
import { usePdfDocument } from '@/components/Pdf/usePdfDocument'

export function PDFViewer({ filePath }: { filePath: string }): React.JSX.Element {
  const { doc, binary, loading, error, progress } = usePdfDocument(filePath)
  if (doc && !loading) return <PdfTab doc={doc} binary={binary} filePath={filePath} />
  const name = fileNameOf(filePath)
  return (
    <div data-testid="pdf-tab-state" className="lv-shell flex h-full w-full flex-col bg-[var(--color-bg)]">
      <LiveHeader fileName={name} fileTitle={filePath} fileGlyph={<SetiFileIcon name={name} size={16} />} sentence={null} />
      <div className="flex flex-1 flex-col items-center justify-center gap-2 bg-[var(--color-bg-inset)] px-6 text-center">
        {error ? (
          <>
            <span role="alert" className="type-row text-[var(--color-status-error-text)]">
              This PDF couldn’t be opened.
            </span>
            <span className="type-hint max-w-[420px] text-[var(--color-text-secondary)]">{error}</span>
          </>
        ) : (
          <span role="status" data-testid="pdf-tab-opening" className="type-row text-[var(--color-text-secondary)]">
            {progress !== null && progress < 1 ? `Opening the PDF… ${Math.floor(progress * 100)}%` : 'Opening the PDF…'}
          </span>
        )}
      </div>
    </div>
  )
}
