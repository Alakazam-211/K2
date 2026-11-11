import { create } from 'zustand'
import { formatDropPaths } from '@/lib/drop-payload'
import { useToastStore } from '@/stores/toast'

export type ClipboardMode = 'copy' | 'cut'

interface FileClipboardState {
  paths: string[]
  mode: ClipboardMode | null
  /** Home M5: the server those paths live on (null: not recorded). A paste
   *  into a room on another server is refused (MS4). */
  hostKey: string | null

  copy: (paths: string[], hostKey?: string) => void
  cut: (paths: string[], hostKey?: string) => void
  clear: () => void
  hasPaths: () => boolean
}

/** What the OS clipboard gets when a copy's names can't go as text. */
export const COPY_AS_TEXT_REFUSAL =
  "Copied in K2 only: a file name has a line break in it, so it can't be pasted as text."

/** Mirror the in-app file clipboard to the OS clipboard so Cmd+V in
 *  the terminal (or any other native app) pastes the file paths, quoted
 *  the same way as a drop (lib/drop-payload.ts). A name with a control
 *  character is not mirrored (a pasted line break would run what follows
 *  it); the tree's own Paste still works from the Zustand state.
 *  Best-effort: a clipboard write failure falls back to in-app only. */
function mirrorToOSClipboard(paths: string[]): void {
  if (paths.length === 0 || typeof navigator === 'undefined') return
  const r = formatDropPaths(paths, { trailingSpace: false })
  if (!r.ok) {
    useToastStore.getState().addToast(COPY_AS_TEXT_REFUSAL, 'warning')
    return
  }
  navigator.clipboard?.writeText(r.text).catch(() => { /* ignored */ })
}

export const useFileClipboardStore = create<FileClipboardState>((set, get) => ({
  paths: [],
  mode: null,
  hostKey: null,

  copy: (paths: string[], hostKey?: string) => {
    set({ paths, mode: 'copy', hostKey: hostKey ?? null })
    mirrorToOSClipboard(paths)
  },

  cut: (paths: string[], hostKey?: string) => {
    set({ paths, mode: 'cut', hostKey: hostKey ?? null })
    mirrorToOSClipboard(paths)
  },

  clear: () => {
    set({ paths: [], mode: null, hostKey: null })
  },

  hasPaths: () => {
    return get().paths.length > 0
  }
}))
