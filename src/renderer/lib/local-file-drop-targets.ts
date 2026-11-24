// Boxes that take OS file drops as paths ON THIS COMPUTER (a Home DM or
// channel's message box: contract-home-chat-attachments-v1). Tauri owns
// every Finder drop (`tauri://drag-drop`), so the external drop router asks
// here first: a point inside a registered element hands it the dropped
// paths as they are, before any upload to a server, whatever server the
// window shows (the box uploads the bytes to its own server itself).

/** Takes the dropped paths (on this computer). */
export type LocalFileDropHandler = (paths: string[]) => void

/** The attribute a registered element carries. */
export const LOCAL_FILE_DROP_ATTR = 'data-local-file-drop'

const targets = new Map<Element, LocalFileDropHandler>()

/** Take drops on `el` until the returned function runs. */
export function registerLocalFileDropTarget(el: HTMLElement, handler: LocalFileDropHandler): () => void {
  targets.set(el, handler)
  el.setAttribute(LOCAL_FILE_DROP_ATTR, '')
  return () => {
    if (targets.get(el) === handler) {
      targets.delete(el)
      el.removeAttribute(LOCAL_FILE_DROP_ATTR)
    }
  }
}

/** The registered box the element under a drop point is in, if any. */
export function localFileDropTargetAt(el: Element | null): LocalFileDropHandler | null {
  let cur: Element | null = el?.closest?.(`[${LOCAL_FILE_DROP_ATTR}]`) ?? null
  while (cur) {
    const h = targets.get(cur)
    if (h) return h
    cur = cur.parentElement?.closest(`[${LOCAL_FILE_DROP_ATTR}]`) ?? null
  }
  return null
}

/** Tests: forget every target. */
export function resetLocalFileDropTargets(): void {
  for (const el of targets.keys()) el.removeAttribute(LOCAL_FILE_DROP_ATTR)
  targets.clear()
}
