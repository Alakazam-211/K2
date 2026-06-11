import { create } from 'zustand'

const HIDDEN_FILES_STORAGE_KEY = 'k2so.showHiddenFiles'

function loadShowHiddenFiles(): boolean {
  try {
    return localStorage.getItem(HIDDEN_FILES_STORAGE_KEY) === 'true'
  } catch { return false }
}

function saveShowHiddenFiles(v: boolean): void {
  try { localStorage.setItem(HIDDEN_FILES_STORAGE_KEY, String(v)) } catch { /* ignore */ }
}

interface FileTreeState {
  showFileTree: boolean
  searchQuery: string
  showHiddenFiles: boolean

  toggleFileTree: () => void
  setShowFileTree: (show: boolean) => void
  setSearchQuery: (query: string) => void
  toggleHiddenFiles: () => void
  setShowHiddenFiles: (show: boolean) => void
}

export const useFileTreeStore = create<FileTreeState>((set) => ({
  showFileTree: false,
  searchQuery: '',
  showHiddenFiles: loadShowHiddenFiles(),

  toggleFileTree: () => set((s) => ({ showFileTree: !s.showFileTree })),
  setShowFileTree: (show: boolean) => set({ showFileTree: show }),
  setSearchQuery: (query: string) => set({ searchQuery: query }),
  toggleHiddenFiles: () => set((s) => {
    const next = !s.showHiddenFiles
    saveShowHiddenFiles(next)
    return { showHiddenFiles: next }
  }),
  setShowHiddenFiles: (show: boolean) => {
    saveShowHiddenFiles(show)
    set({ showHiddenFiles: show })
  },
}))
