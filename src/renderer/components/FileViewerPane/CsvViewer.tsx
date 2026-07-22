// CSV/TSV table preview for FileViewerPane. Parent owns text load + raw
// edit via CodeEditor; this component only renders the Preview table.

import { useMemo } from 'react'
import { parseDelimited, CSV_PREVIEW_MAX_ROWS } from './csvParse'
import { isTsvPath } from './fileCategory'

interface CsvViewerProps {
  content: string
  filePath: string
}

export function CsvViewer({ content, filePath }: CsvViewerProps): React.JSX.Element {
  const delimiter = isTsvPath(filePath) ? '\t' : ','
  const kind = isTsvPath(filePath) ? 'TSV' : 'CSV'

  const { rows, truncated } = useMemo(
    () =>
      parseDelimited(content, {
        delimiter,
        maxRows: CSV_PREVIEW_MAX_ROWS,
      }),
    [content, delimiter],
  )

  if (rows.length === 0) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[var(--color-bg)] text-sm text-[var(--color-text-muted)]">
        Empty {kind} file
      </div>
    )
  }

  const colCount = rows.reduce((max, r) => Math.max(max, r.length), 0)
  const header = rows[0]
  const body = rows.slice(1)

  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-[var(--color-bg)]">
      {truncated && (
        <div className="flex-shrink-0 border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 py-1.5 text-[11px] text-[var(--color-text-secondary)]">
          Showing first {CSV_PREVIEW_MAX_ROWS.toLocaleString()} rows (truncated). Switch to{' '}
          <span className="font-medium text-[var(--color-text-primary)]">Edit</span> for the full
          file.
        </div>
      )}
      <div className="flex-1 overflow-auto">
        <table className="w-max min-w-full border-collapse text-left text-[11px] font-mono">
          <thead className="sticky top-0 z-10 bg-[var(--color-bg-stripe)]">
            <tr>
              <th className="sticky left-0 z-20 border-b border-r border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-2 py-1 text-[10px] font-medium text-[var(--color-text-muted)]">
                #
              </th>
              {Array.from({ length: colCount }, (_, c) => (
                <th
                  key={c}
                  className="border-b border-r border-[var(--color-border)] px-2 py-1 text-[10px] font-medium text-[var(--color-text-muted)] whitespace-nowrap"
                >
                  {header[c] ?? ''}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {body.map((row, ri) => (
              <tr
                key={ri}
                className="odd:bg-[var(--color-bg)] even:bg-[var(--color-bg-stripe)]/40 hover:bg-[var(--color-accent)]/10"
              >
                <td className="sticky left-0 z-10 border-r border-[var(--color-border)] bg-[inherit] px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] tabular-nums">
                  {ri + 2}
                </td>
                {Array.from({ length: colCount }, (_, c) => (
                  <td
                    key={c}
                    className="border-r border-[var(--color-border)]/50 px-2 py-0.5 text-[var(--color-text-primary)] whitespace-pre max-w-[28rem] truncate"
                    title={row[c] ?? ''}
                  >
                    {row[c] ?? ''}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="flex-shrink-0 border-t border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 py-0.5 text-[10px] font-mono text-[var(--color-text-muted)]">
        {rows.length.toLocaleString()} row{rows.length === 1 ? '' : 's'}
        {truncated ? '+' : ''} · {colCount} col{colCount === 1 ? '' : 's'} · {kind}
      </div>
    </div>
  )
}
