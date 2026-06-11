import { create } from 'zustand'

// Promise-based store for the RemoteFolderPicker modal. Mirrors the
// open()/resolve() idiom of add-workspace-dialog so a caller can simply
// `const path = await useRemoteFolderPickerStore.getState().open()` and
// get back the chosen REMOTE directory path (or null on cancel).
//
// The picker browses the active remote daemon's filesystem via the
// host-aware `daemonCliGet` (see RemoteFolderPicker.tsx), so this store
// holds no fs state — only the open/closed flag and the pending resolver.

interface RemoteFolderPickerState {
  isOpen: boolean
  /** Internal: resolves the promise returned by `open()`. */
  resolver: ((path: string | null) => void) | null

  /** Open the picker and await the chosen folder path (null on cancel). */
  open: () => Promise<string | null>
  /** Resolve with the chosen path and close. */
  select: (path: string) => void
  /** Resolve with null and close. */
  cancel: () => void
}

export const useRemoteFolderPickerStore = create<RemoteFolderPickerState>((set, get) => ({
  isOpen: false,
  resolver: null,

  open: () =>
    new Promise<string | null>((resolve) => {
      // If a picker is somehow already open, cancel it first so its
      // promise settles rather than leaking.
      const prev = get().resolver
      if (prev) prev(null)
      set({ isOpen: true, resolver: resolve })
    }),

  select: (path: string) => {
    const resolve = get().resolver
    set({ isOpen: false, resolver: null })
    if (resolve) resolve(path)
  },

  cancel: () => {
    const resolve = get().resolver
    set({ isOpen: false, resolver: null })
    if (resolve) resolve(null)
  },
}))
