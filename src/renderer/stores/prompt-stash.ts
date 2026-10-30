// Prompt stash (spec-prompt-stash-v1, 0.46.7): the person's saved drafts on
// this device. The working copy of `lib/prompt-stash/idb.ts`.
//
// Like Home (`stores/homes.ts`), this store must NEVER subscribe to
// `onActiveHostChange`: a server switch never changes the list (acceptance
// 5). Each item remembers the server and person it was written under, for
// display only. It runs no loop, so a headless daemon misses nothing; and
// agents never read or write it (it is the person's own drafts).
//
// Windows share one origin, so one IndexedDB: a change posts on a
// BroadcastChannel and the other windows read the list again.

import { create } from 'zustand'
import {
  buildStashRecord,
  expiredTombstones,
  liveNewestFirst,
  newStashId,
  oldestLive,
  STASH_CAP,
  STASH_FULL_MESSAGE,
  tombstone,
  type StashAttachment,
  type StashDraft,
  type StashFileInput,
  type StashRecord,
} from '@/lib/prompt-stash/record'
import { readAllStashRecords, readStashBlob, writeStash, type StashBlobRow, type StashWrite } from '@/lib/prompt-stash/idb'
import { stashDeviceId } from '@/lib/prompt-stash/device'
import { noteLocalStashChange } from '@/lib/prompt-stash/sync-runner'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

function syncKv(): Storage | null {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

let localChangeListener: (() => void) | null = null

/** stash-sync-v1: the sync runner (in the window that runs it) hears
 *  about this window's saves and deletes. */
export function setStashLocalChangeListener(fn: (() => void) | null): void {
  localChangeListener = fn
}

/** A local save or delete: queue it for sync (every window shares the
 *  queue in localStorage; the runner's window also hears `storage`). */
function noteChanged(records: ReadonlyArray<Pick<StashRecord, 'id' | 'updated_at'>>): void {
  noteLocalStashChange(syncKv(), records)
  localChangeListener?.()
}

/** Device setting: remove an item once it's used. Off by default. */
export const STASH_REMOVE_AFTER_USE_KEY = 'k2.promptStash.removeAfterUse'
const CHANNEL = 'k2.prompt-stash'

export type StashSaveResult =
  | { status: 'saved'; record: StashRecord }
  /** At the cap: nothing was written. Ask, then save with `removeOldest`. */
  | { status: 'full'; oldest: StashRecord }

export interface StashSaveOptions {
  /** The person said yes to "Remove the oldest?". */
  removeOldest?: boolean
  /** Clock (tests). */
  now?: number
}

interface PromptStashState {
  /** Live items, newest first. */
  items: StashRecord[]
  loaded: boolean
  removeAfterUse: boolean
  load: () => Promise<void>
  save: (draft: StashDraft, files?: StashFileInput[], opts?: StashSaveOptions) => Promise<StashSaveResult>
  remove: (id: string) => Promise<void>
  /** An item was put into a composer: with "Remove after use" on, it goes. */
  markUsed: (id: string) => Promise<void>
  setRemoveAfterUse: (on: boolean) => void
}

function readRemoveAfterUse(): boolean {
  try {
    return localStorage.getItem(STASH_REMOVE_AFTER_USE_KEY) === '1'
  } catch {
    return false
  }
}

/** A Blob's bytes (FileReader where `arrayBuffer()` is missing). */
function blobBytes(b: Blob): Promise<ArrayBuffer> {
  if (typeof b.arrayBuffer === 'function') return b.arrayBuffer()
  return new Promise((resolve, reject) => {
    const r = new FileReader()
    r.onload = () => resolve(r.result as ArrayBuffer)
    r.onerror = () => reject(r.error ?? new Error('Could not read the file'))
    r.readAsArrayBuffer(b)
  })
}

let channel: BroadcastChannel | null = null
function getChannel(): BroadcastChannel | null {
  if (channel) return channel
  if (typeof BroadcastChannel === 'undefined') return null
  try {
    channel = new BroadcastChannel(CHANNEL)
    channel.onmessage = () => {
      void usePromptStashStore.getState().load()
    }
  } catch {
    channel = null
  }
  return channel
}
function announce(): void {
  try {
    getChannel()?.postMessage('changed')
  } catch {
    /* another window just reads it on its next load */
  }
}

// Writes run one at a time: the cap check and the write see the same list.
let queue: Promise<unknown> = Promise.resolve()
function serial<T>(fn: () => Promise<T>): Promise<T> {
  const run = queue.then(fn, fn)
  queue = run.catch(() => undefined)
  return run
}

export const usePromptStashStore = create<PromptStashState>((set, get) => ({
  items: [],
  loaded: false,
  removeAfterUse: readRemoveAfterUse(),

  load: async () => {
    getChannel()
    const all = await readAllStashRecords()
    set({ items: liveNewestFirst(all), loaded: true })
  },

  save: (draft, files = [], opts = {}) =>
    serial(async () => {
      const now = opts.now ?? Date.now()
      const all = await readAllStashRecords()
      const live = liveNewestFirst(all)
      let evict: StashRecord | null = null
      if (live.length >= STASH_CAP) {
        const oldest = oldestLive(all)
        if (!oldest) throw new Error('stash is full but has no oldest item')
        if (!opts.removeOldest) return { status: 'full', oldest } as const
        evict = oldest
      }
      // stash-sync-v1: each file records the device that holds it and its
      // path there; the bytes stay on this device, by attachment id.
      const deviceId = stashDeviceId()
      const attachments: StashAttachment[] = []
      const blobs: StashBlobRow[] = []
      for (const f of files) {
        const id = newStashId()
        const data = await blobBytes(f.bytes)
        attachments.push({ id, name: f.name, type: f.type, size: data.byteLength, device_id: deviceId, path: f.path ?? null })
        blobs.push({ id, type: f.type, data })
      }
      const record = buildStashRecord(draft, attachments, now, newStashId(), deviceId)
      const evicted = evict ? tombstone(evict, now, deviceId) : null
      const expired = expiredTombstones(all, now).map((r) => r.id)
      await writeStash({
        put: evicted ? [record, evicted] : [record],
        putBlobs: blobs,
        deleteRecords: expired,
        deleteBlobs: evict ? evict.attachments.map((a) => a.id) : [],
      })
      noteChanged(evicted ? [record, evicted] : [record])
      await get().load()
      announce()
      return { status: 'saved', record } as const
    }),

  remove: (id) =>
    serial(async () => {
      const now = Date.now()
      const all = await readAllStashRecords()
      const rec = all.find((r) => r.id === id)
      if (!rec || rec.deleted_at !== null) return
      const t = tombstone(rec, now, stashDeviceId())
      await writeStash({ put: [t], deleteBlobs: rec.attachments.map((a) => a.id) })
      noteChanged([t])
      await get().load()
      announce()
    }),

  markUsed: async (id) => {
    if (!get().removeAfterUse) return
    await get().remove(id)
  },

  setRemoveAfterUse: (on) => {
    try {
      if (on) localStorage.setItem(STASH_REMOVE_AFTER_USE_KEY, '1')
      else localStorage.removeItem(STASH_REMOVE_AFTER_USE_KEY)
    } catch {
      /* kept for this window only */
    }
    set({ removeAfterUse: on })
  },
}))

/** stash-sync-v1: merge what a pull brought, in the same queue as saves
 *  (the cap check and a merge never interleave). `fn` sees a fresh read of
 *  every record and returns the write (null: nothing to do). */
export function applyStashSync(fn: (all: StashRecord[]) => StashWrite | null): Promise<void> {
  return serial(async () => {
    const w = fn(await readAllStashRecords())
    if (!w) return
    await writeStash(w)
    await usePromptStashStore.getState().load()
    announce()
  })
}

/** An attachment's bytes as a File (to upload into a composer), or null
 *  when its bytes are gone. */
export async function readStashFile(att: StashAttachment): Promise<File | null> {
  const row = await readStashBlob(att.id)
  if (!row) return null
  return new File([row.data], att.name, { type: row.type || att.type || 'application/octet-stream' })
}

export type StashAskResult = StashSaveResult | { status: 'cancelled' }

/** Save, and at the cap ask "Remove the oldest?" first. Never drops
 *  anything without a yes (spec "Cap: 50 stashes"). */
export async function saveToStash(
  draft: StashDraft,
  files: StashFileInput[] = [],
  ask: () => Promise<boolean> = askRemoveOldest,
): Promise<StashAskResult> {
  const store = usePromptStashStore.getState()
  const first = await store.save(draft, files)
  if (first.status === 'saved') return first
  if (!(await ask())) return { status: 'cancelled' }
  return store.save(draft, files, { removeOldest: true })
}

export function askRemoveOldest(): Promise<boolean> {
  return useConfirmDialogStore.getState().confirm({
    title: 'Stash full',
    message: STASH_FULL_MESSAGE,
    confirmLabel: 'Remove oldest',
    destructive: true,
  })
}
