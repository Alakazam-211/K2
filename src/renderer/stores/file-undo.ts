import { create } from 'zustand'

const MAX_UNDO_STACK = 50

export type FileOperation =
  | { type: 'create'; path: string }
  | { type: 'delete'; paths: string[]; note: string }
  | { type: 'rename'; oldPath: string; newPath: string }
  | { type: 'move'; items: Array<{ oldPath: string; newPath: string }> }
  | { type: 'copy'; createdPaths: string[] }

interface FileUndoState {
  stack: FileOperation[]

  push: (op: FileOperation) => void
  pop: () => FileOperation | undefined
  clear: () => void
  canUndo: () => boolean
}

export const useFileUndoStore = create<FileUndoState>((set, get) => ({
  stack: [],

  push: (op: FileOperation) => {
    set((state) => {
      const next = [...state.stack, op]
      if (next.length > MAX_UNDO_STACK) {
        next.shift()
      }
      return { stack: next }
    })
  },

  pop: () => {
    const { stack } = get()
    if (stack.length === 0) return undefined
    const op = stack[stack.length - 1]
    set({ stack: stack.slice(0, -1) })
    return op
  },

  clear: () => {
    set({ stack: [] })
  },

  canUndo: () => {
    return get().stack.length > 0
  }
}))
