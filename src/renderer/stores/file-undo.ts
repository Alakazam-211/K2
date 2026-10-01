import { create } from 'zustand'

const MAX_UNDO_STACK = 50

export type FileOperation = (
  | { type: 'create'; path: string }
  | { type: 'delete'; paths: string[]; note: string }
  | { type: 'rename'; oldPath: string; newPath: string }
  | { type: 'move'; items: Array<{ oldPath: string; newPath: string }> }
  | { type: 'copy'; createdPaths: string[] }
) & {
  /** Home M5: the server whose files the operation touched (`local` when
   *  untagged). Undo in a room only ever reverses that room's server's
   *  operations: a path from one server means nothing on another (MS4). */
  hostKey?: string
}

interface FileUndoState {
  stack: FileOperation[]

  push: (op: FileOperation) => void
  pop: () => FileOperation | undefined
  /** Pop the newest operation on `hostKey`'s files (others stay). */
  popFor: (hostKey: string) => FileOperation | undefined
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

  popFor: (hostKey: string) => {
    const { stack } = get()
    for (let i = stack.length - 1; i >= 0; i--) {
      if ((stack[i].hostKey ?? 'local') !== hostKey) continue
      const op = stack[i]
      set({ stack: [...stack.slice(0, i), ...stack.slice(i + 1)] })
      return op
    }
    return undefined
  },

  clear: () => {
    set({ stack: [] })
  },

  canUndo: () => {
    return get().stack.length > 0
  }
}))

/** The undo stack as one room's file tree uses it: pushes are tagged with
 *  the room's server, and undo pops only that server's operations. */
export function fileUndoFor(hostKey: string): { push: (op: FileOperation) => void; pop: () => FileOperation | undefined } {
  const s = useFileUndoStore.getState()
  return {
    push: (op) => s.push({ ...op, hostKey }),
    pop: () => s.popFor(hostKey),
  }
}
