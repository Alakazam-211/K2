// Prompt stash storage: IndexedDB on this device (spec-prompt-stash-v1,
// "Where it's stored"). Not localStorage: attachments would overflow it.
// Like Home, it belongs to the person on this computer, not to a server:
// nothing here knows which server the window is on.
//
// Two object stores in one database:
//   items  — the shared records (`record.ts`), keyPath `id`;
//   blobs  — attachment bytes by attachment id, never inside a record.
// Every change is one transaction over both, and a write only counts once
// the transaction completes (`oncomplete`), so the composer clears only
// after the stash is on disk.

import { coerceStashRecord, type StashRecord } from './record'

export const STASH_DB_NAME = 'k2.prompt-stash'
export const STASH_DB_VERSION = 1
const ITEMS = 'items'
const BLOBS = 'blobs'

export interface StashBlobRow {
  id: string
  type: string
  data: ArrayBuffer
}

/** One transaction's worth of changes. */
export interface StashWrite {
  put?: StashRecord[]
  putBlobs?: StashBlobRow[]
  deleteRecords?: string[]
  deleteBlobs?: string[]
}

function idb(): IDBFactory {
  const f = (globalThis as { indexedDB?: IDBFactory }).indexedDB
  if (!f) throw new Error('This window has no IndexedDB, so the stash cannot be saved.')
  return f
}

function req<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result)
    r.onerror = () => reject(r.error ?? new Error('IndexedDB request failed'))
  })
}

let opening: Promise<IDBDatabase> | null = null

export function openStashDb(): Promise<IDBDatabase> {
  if (opening) return opening
  const p = new Promise<IDBDatabase>((resolve, reject) => {
    const r = idb().open(STASH_DB_NAME, STASH_DB_VERSION)
    r.onupgradeneeded = () => {
      const db = r.result
      if (!db.objectStoreNames.contains(ITEMS)) db.createObjectStore(ITEMS, { keyPath: 'id' })
      if (!db.objectStoreNames.contains(BLOBS)) db.createObjectStore(BLOBS, { keyPath: 'id' })
    }
    r.onsuccess = () => {
      const db = r.result
      // Another window upgrading later: let it.
      db.onversionchange = () => {
        db.close()
        opening = null
      }
      resolve(db)
    }
    r.onerror = () => reject(r.error ?? new Error('Could not open the stash database'))
    r.onblocked = () => reject(new Error('The stash database is held by another window'))
  })
  opening = p
  p.catch(() => {
    if (opening === p) opening = null
  })
  return p
}

/** Close and forget the open handle (tests: a "reload"). */
export async function closeStashDbForTests(): Promise<void> {
  const p = opening
  opening = null
  if (p) (await p).close()
}

export async function readAllStashRecords(): Promise<StashRecord[]> {
  const db = await openStashDb()
  const tx = db.transaction(ITEMS, 'readonly')
  const rows = await req(tx.objectStore(ITEMS).getAll())
  return rows.flatMap((row) => {
    const rec = coerceStashRecord(row)
    return rec ? [rec] : []
  })
}

/** Apply `w` in one transaction; resolves once it is committed. */
export async function writeStash(w: StashWrite): Promise<void> {
  const db = await openStashDb()
  const tx = db.transaction([ITEMS, BLOBS], 'readwrite')
  const done = new Promise<void>((resolve, reject) => {
    tx.oncomplete = () => resolve()
    tx.onerror = () => reject(tx.error ?? new Error('The stash write failed'))
    tx.onabort = () => reject(tx.error ?? new Error('The stash write was aborted'))
  })
  const items = tx.objectStore(ITEMS)
  const blobs = tx.objectStore(BLOBS)
  for (const b of w.putBlobs ?? []) blobs.put(b)
  for (const r of w.put ?? []) items.put(r)
  for (const id of w.deleteRecords ?? []) items.delete(id)
  for (const id of w.deleteBlobs ?? []) blobs.delete(id)
  await done
}

export async function readStashBlob(id: string): Promise<StashBlobRow | null> {
  const db = await openStashDb()
  const tx = db.transaction(BLOBS, 'readonly')
  const row = (await req(tx.objectStore(BLOBS).get(id))) as StashBlobRow | undefined
  return row && row.data ? row : null
}
