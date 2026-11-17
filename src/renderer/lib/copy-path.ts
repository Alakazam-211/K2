// "Copy path" everywhere it is offered (a tab's menu, the file tree, the
// workspace list): which path a tab stands for, and the copy itself.
//
// The copy goes through the Tauri clipboard plugin, with navigator.clipboard
// as the fallback (the web client). These copies run after a context menu's
// await, and the desktop webview refuses a web clipboard write it does not
// count as part of a click, so navigator.clipboard alone failed there, and
// quietly. Every copy says how it went: "Path copied", or why not.

import { copyToClipboard } from '@/components/common/chat-copy'
import { useToastStore } from '@/stores/toast'
import type { FileViewerItemData, Tab } from '@/stores/tabs'

/** The path of the file a tab shows, as its viewer header names it (the
 *  path on the server the file lives on), or null for a tab with no file
 *  (agents, chat, terminals, browsers). A pane that holds several files
 *  shows its selected one; a split tab answers for its first pane showing a
 *  file. */
export function tabFilePath(tab: Pick<Tab, 'paneGroups'>): string | null {
  for (const pg of tab.paneGroups.values()) {
    const item = pg.items[pg.activeItemIndex] ?? pg.items[0]
    if (item?.type !== 'file-viewer') continue
    const path = (item.data as FileViewerItemData).filePath
    if (path) return path
  }
  return null
}

/** Copy `text` and say so in a toast (`done`), or say why it failed. */
export async function copyWithToast(text: string, done: string, failed: string): Promise<void> {
  try {
    await copyToClipboard(text)
  } catch (err) {
    useToastStore.getState().addToast(`${failed}: ${err instanceof Error ? err.message : String(err)}`, 'error')
    return
  }
  useToastStore.getState().addToast(done, 'success', 2000)
}

/** Copy a file or folder path: "Path copied". */
export function copyPathWithToast(path: string): Promise<void> {
  return copyWithToast(path, 'Path copied', "Couldn't copy the path")
}
