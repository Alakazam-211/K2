import { create } from 'zustand'

export type ClipboardMode = 'copy' | 'cut'

interface FileClipboardState {
  paths: string[]
  mode: ClipboardMode | null

  copy: (paths: string[]) => void
  cut: (paths: string[]) => void
  clear: () => void
  hasPaths: () => boolean
}

/**
 * Shell-escape a path for safe pasting into a terminal command line.
 * Mirrors `lib/file-drag.ts::shellEscape` so dragged-paths and
 * copied-paths produce the same on-clipboard representation.
 *
 * Backslash-style escape would break Claude Code's image detector
 * (it strips quotes then `fs.exists()`s the path), but for plain
 * file paths through Cmd+C → Cmd+V the user almost always wants the
 * literal text in the terminal — single-quoting is the conservative
 * choice that survives shells, the image detector, and copy/paste
 * roundtrips.
 */
function shellEscape(p: string): string {
  if (/[^a-zA-Z0-9_\-./]/.test(p)) {
    return "'" + p.replace(/'/g, "'\\''") + "'"
  }
  return p
}

/** Mirror the in-app file clipboard to the OS clipboard so Cmd+V in
 *  the terminal (or any other native app) pastes the file paths.
 *  Best-effort: failures silently fall back to in-app-only behavior
 *  since the tree's own Paste action still works from the Zustand
 *  state. */
function mirrorToOSClipboard(paths: string[]): void {
  if (paths.length === 0 || typeof navigator === 'undefined') return
  const text = paths.map(shellEscape).join(' ')
  navigator.clipboard?.writeText(text).catch(() => { /* ignored */ })
}

export const useFileClipboardStore = create<FileClipboardState>((set, get) => ({
  paths: [],
  mode: null,

  copy: (paths: string[]) => {
    set({ paths, mode: 'copy' })
    mirrorToOSClipboard(paths)
  },

  cut: (paths: string[]) => {
    set({ paths, mode: 'cut' })
    mirrorToOSClipboard(paths)
  },

  clear: () => {
    set({ paths: [], mode: null })
  },

  hasPaths: () => {
    return get().paths.length > 0
  }
}))
