import { create } from 'zustand'

// Promise-based store for the RemoteFolderPicker modal. Mirrors the
// open()/resolve() idiom of add-workspace-dialog so a caller can simply
// `const path = await useRemoteFolderPickerStore.getState().open()` and
// get back the chosen REMOTE directory path (or null on cancel).
//
// The picker browses the active remote daemon's filesystem via the
// host-aware `daemonCliGet` (see RemoteFolderPicker.tsx), so this store
// holds no fs state — only the open/closed flag, the pending resolver,
// and the per-open options.
//
// Two modes:
//   - 'folder' (default, backward-compatible): browse directories only,
//     "Select this folder" resolves with the current directory.
//   - 'file': directories remain navigable for traversal, but files are
//     ALSO listed (optionally filtered by `accept`), and clicking a file
//     resolves with that file's full remote path. Used e.g. to pick an
//     image on the remote host for a project/workspace icon.

export interface RemoteFolderPickerOptions {
  /** 'folder' (default) or 'file' — see module comment. */
  mode?: 'folder' | 'file'
  /** File-mode only: keep a file in the listing when this returns true
   *  for its name. Omit to list every file. Ignored in folder mode. */
  accept?: (name: string) => boolean
  /** Override the modal title (defaults per mode). */
  title?: string
}

interface RemoteFolderPickerState {
  isOpen: boolean
  /** Normalized from `open()` opts — 'folder' when opened with no opts. */
  mode: 'folder' | 'file'
  /** File-mode filename filter (null = list all files). */
  accept: ((name: string) => boolean) | null
  /** Title override (null = per-mode default). */
  title: string | null
  /** Internal: resolves the promise returned by `open()`. */
  resolver: ((path: string | null) => void) | null

  /** Open the picker and await the chosen path (null on cancel).
   *  No opts = folder mode, identical to the historical behavior. */
  open: (opts?: RemoteFolderPickerOptions) => Promise<string | null>
  /** Resolve with the chosen path and close. */
  select: (path: string) => void
  /** Resolve with null and close. */
  cancel: () => void
}

export const useRemoteFolderPickerStore = create<RemoteFolderPickerState>((set, get) => ({
  isOpen: false,
  mode: 'folder',
  accept: null,
  title: null,
  resolver: null,

  open: (opts?: RemoteFolderPickerOptions) =>
    new Promise<string | null>((resolve) => {
      // If a picker is somehow already open, cancel it first so its
      // promise settles rather than leaking.
      const prev = get().resolver
      if (prev) prev(null)
      set({
        isOpen: true,
        mode: opts?.mode ?? 'folder',
        accept: opts?.accept ?? null,
        title: opts?.title ?? null,
        resolver: resolve,
      })
    }),

  select: (path: string) => {
    const resolve = get().resolver
    set({ isOpen: false, mode: 'folder', accept: null, title: null, resolver: null })
    if (resolve) resolve(path)
  },

  cancel: () => {
    const resolve = get().resolver
    set({ isOpen: false, mode: 'folder', accept: null, title: null, resolver: null })
    if (resolve) resolve(null)
  },
}))
