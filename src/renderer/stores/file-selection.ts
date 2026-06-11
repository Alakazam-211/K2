import { create } from 'zustand'

interface FileSelectionState {
  selectedPaths: Record<string, true>
  lastSelectedPath: string | null

  select: (path: string) => void
  toggleSelect: (path: string) => void
  rangeSelect: (path: string, allPaths: string[]) => void
  selectAll: (paths: string[]) => void
  clearSelection: () => void
  isSelected: (path: string) => boolean
  getSelectedPaths: () => string[]
}

export const useFileSelectionStore = create<FileSelectionState>((set, get) => ({
  selectedPaths: {},
  lastSelectedPath: null,

  select: (path: string) => {
    set({ selectedPaths: { [path]: true }, lastSelectedPath: path })
  },

  toggleSelect: (path: string) => {
    set((state) => {
      const next = { ...state.selectedPaths }
      if (next[path]) {
        delete next[path]
      } else {
        next[path] = true
      }
      return { selectedPaths: next, lastSelectedPath: path }
    })
  },

  rangeSelect: (path: string, allPaths: string[]) => {
    const { lastSelectedPath } = get()
    if (!lastSelectedPath) {
      set({ selectedPaths: { [path]: true }, lastSelectedPath: path })
      return
    }

    const startIndex = allPaths.indexOf(lastSelectedPath)
    const endIndex = allPaths.indexOf(path)

    if (startIndex === -1 || endIndex === -1) {
      set({ selectedPaths: { [path]: true }, lastSelectedPath: path })
      return
    }

    const low = Math.min(startIndex, endIndex)
    const high = Math.max(startIndex, endIndex)
    const next: Record<string, true> = {}
    for (let i = low; i <= high; i++) {
      next[allPaths[i]] = true
    }

    set({ selectedPaths: next, lastSelectedPath: path })
  },

  selectAll: (paths: string[]) => {
    const next: Record<string, true> = {}
    for (const p of paths) {
      next[p] = true
    }
    set({ selectedPaths: next })
  },

  clearSelection: () => {
    set({ selectedPaths: {}, lastSelectedPath: null })
  },

  isSelected: (path: string) => {
    return !!get().selectedPaths[path]
  },

  getSelectedPaths: () => {
    return Object.keys(get().selectedPaths)
  }
}))
