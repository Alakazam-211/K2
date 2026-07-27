/**
 * Normalize `GET /cli/fs/read-dir` JSON into a directory-entry array.
 *
 * The daemon ships a bare JSON array (`DirEntryInfo[]`). Call sites that
 * do `[...raw].sort(...)` or `raw.filter(...)` crash with
 * "Spread syntax requires ...iterable[Symbol.iterator] to be a function"
 * (Bun/WebKit) when `raw` is accidentally a plain object or undefined —
 * e.g. empty body after auth recovery, a wrapped future shape, or a
 * mis-routed payload. Centralize the coerce so every tree/picker is safe.
 */

export interface FsDirEntry {
  name: string
  path: string
  isDirectory: boolean
  size?: number
  modifiedAt?: number
}

/** Coerce an unknown `fs/read-dir` body to an entry array, or throw. */
export function normalizeFsReadDir(raw: unknown): FsDirEntry[] {
  if (Array.isArray(raw)) return raw as FsDirEntry[]
  if (
    raw !== null &&
    typeof raw === 'object' &&
    Array.isArray((raw as { entries?: unknown }).entries)
  ) {
    return (raw as { entries: FsDirEntry[] }).entries
  }
  const kind =
    raw === null || raw === undefined
      ? String(raw)
      : Array.isArray(raw)
        ? 'array'
        : typeof raw
  throw new Error(`fs/read-dir returned non-array (${kind})`)
}
