// Lightweight CSV/TSV parser for the file-viewer table preview.
// RFC4180-ish: handles quoted fields, doubled quotes, and CRLF.
// Not a full streaming CSV library — good enough for agent artifacts.

export interface ParseDelimitedOptions {
  /** Field separator. Default comma. Use '\t' for TSV. */
  delimiter?: string
  /** Max data rows to return (excluding a header-looking first row). 0 = unlimited. */
  maxRows?: number
}

export interface ParseDelimitedResult {
  /** All rows (including header if present). Truncated when maxRows is set. */
  rows: string[][]
  /** True when input had more rows than maxRows allowed. */
  truncated: boolean
}

/**
 * Parse a delimited text blob into a 2D string array.
 * Empty input → zero rows. Does not infer headers.
 */
export function parseDelimited(
  text: string,
  options: ParseDelimitedOptions = {},
): ParseDelimitedResult {
  const delimiter = options.delimiter ?? ','
  const maxRows = options.maxRows ?? 0

  const rows: string[][] = []
  let field = ''
  let row: string[] = []
  let inQuotes = false
  let i = 0
  let truncated = false
  const len = text.length

  const pushRow = (): boolean => {
    // Drop a trailing empty row produced by a final newline only when
    // the row is completely empty and we already have data.
    if (row.length === 1 && row[0] === '' && rows.length > 0 && !inQuotes) {
      row = []
      field = ''
      return true
    }
    if (maxRows > 0 && rows.length >= maxRows) {
      truncated = true
      row = []
      field = ''
      return false
    }
    row.push(field)
    rows.push(row)
    row = []
    field = ''
    return true
  }

  while (i < len) {
    if (truncated) break

    const ch = text[i]

    if (inQuotes) {
      if (ch === '"') {
        if (i + 1 < len && text[i + 1] === '"') {
          field += '"'
          i += 2
          continue
        }
        inQuotes = false
        i += 1
        continue
      }
      field += ch
      i += 1
      continue
    }

    if (ch === '"') {
      inQuotes = true
      i += 1
      continue
    }

    if (ch === delimiter) {
      row.push(field)
      field = ''
      i += 1
      continue
    }

    if (ch === '\n') {
      if (!pushRow()) break
      i += 1
      continue
    }

    if (ch === '\r') {
      // CRLF or bare CR
      if (i + 1 < len && text[i + 1] === '\n') i += 1
      if (!pushRow()) break
      i += 1
      continue
    }

    field += ch
    i += 1
  }

  // Final field/row (file without trailing newline)
  if (!truncated && (field.length > 0 || row.length > 0 || inQuotes)) {
    if (!(row.length === 0 && field === '' && rows.length > 0)) {
      pushRow()
    }
  }

  return { rows, truncated }
}

/** Default preview row cap for the table view (PRD: ~5k). */
export const CSV_PREVIEW_MAX_ROWS = 5000
