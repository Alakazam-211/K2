// Unsaved words, kept on this computer (build-editors.md: drafts and the
// restore bar). Each plain file's unsaved buffer is written here as it
// changes and deleted once a save lands, so a quit, a crash or a failed save
// never takes your words with it. On open, a draft that differs from the file
// comes back ("Restored your unsaved changes from before K2 quit.").
//
//   - Key: (server host key, absolute path). Per computer: this is a view of
//     your own unsaved words, not the daemon's truth (a live doc's offline
//     edits are the kit's).
//   - IndexedDB (never localStorage: file bodies), one small database. The
//     base the draft was typed on is kept too, so a file changed while K2
//     was closed is merged, not overwritten.
//   - Every call is wrapped: a window without IndexedDB, or a failing one,
//     keeps drafts in memory for this window only.
//   - Versioned (`v`): a row of another version is ignored.

const DB_NAME = 'k2.editor-drafts'
const DB_VERSION = 1
const STORE = 'drafts'
export const DRAFT_VERSION = 1
/** Drafts older than this are dropped on read (a month). */
export const DRAFT_MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000

export interface Draft {
  key: string
  hostKey: string
  path: string
  /** The unsaved buffer. */
  text: string
  /** The file's text it was typed on; null when the file didn't exist. */
  base: string | null
  savedAt: number
  v: number
}

export const draftKey = (hostKey: string, path: string): string => `${hostKey}\u0000${path}`

const memory = new Map<string, Draft>()
let opening: Promise<IDBDatabase> | null = null

function factory(): IDBFactory | null {
  try {
    return typeof indexedDB === 'undefined' ? null : indexedDB
  } catch {
    return null
  }
}

function openDb(): Promise<IDBDatabase> | null {
  const f = factory()
  if (!f) return null
  if (opening) return opening
  opening = new Promise<IDBDatabase>((resolve, reject) => {
    const r = f.open(DB_NAME, DB_VERSION)
    r.onupgradeneeded = () => {
      if (!r.result.objectStoreNames.contains(STORE)) r.result.createObjectStore(STORE, { keyPath: 'key' })
    }
    r.onsuccess = () => resolve(r.result)
    r.onerror = () => reject(r.error)
    r.onblocked = () => reject(new Error('editor drafts: database blocked'))
  }).catch((err) => {
    opening = null
    throw err
  })
  return opening
}

function request<T>(mode: IDBTransactionMode, run: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> | null {
  const db = openDb()
  if (!db) return null
  return db.then(
    (d) =>
      new Promise<T>((resolve, reject) => {
        const tx = d.transaction(STORE, mode)
        const req = run(tx.objectStore(STORE))
        tx.oncomplete = () => resolve(req.result)
        tx.onerror = () => reject(tx.error)
        tx.onabort = () => reject(tx.error)
      }),
  )
}

export async function putDraft(hostKey: string, path: string, text: string, base: string | null): Promise<void> {
  const row: Draft = { key: draftKey(hostKey, path), hostKey, path, text, base, savedAt: Date.now(), v: DRAFT_VERSION }
  memory.set(row.key, row)
  try {
    await request('readwrite', (s) => s.put(row))
  } catch {
    /* memory keeps it for this window */
  }
}

export async function getDraft(hostKey: string, path: string): Promise<Draft | null> {
  const key = draftKey(hostKey, path)
  let row: Draft | null = memory.get(key) ?? null
  try {
    const stored = await request<Draft | undefined>('readonly', (s) => s.get(key) as IDBRequest<Draft | undefined>)
    if (stored && (!row || stored.savedAt >= row.savedAt)) row = stored
  } catch {
    /* memory only */
  }
  if (!row || row.v !== DRAFT_VERSION || typeof row.text !== 'string') return null
  if (Date.now() - row.savedAt > DRAFT_MAX_AGE_MS) {
    void clearDraft(hostKey, path)
    return null
  }
  return row
}

export async function clearDraft(hostKey: string, path: string): Promise<void> {
  const key = draftKey(hostKey, path)
  memory.delete(key)
  try {
    await request('readwrite', (s) => s.delete(key))
  } catch {
    /* nothing kept */
  }
}

/** Tests: forget the memory copies and the open handle. */
export function resetDraftsForTests(): void {
  memory.clear()
  opening = null
}
