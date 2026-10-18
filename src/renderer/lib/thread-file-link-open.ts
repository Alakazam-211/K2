// What a Thread file link does when clicked (Thread file links, Rosson
// 2026-10-18). The daemon already said the path is a file or folder inside
// the Thread's workspace (`components/common/file-links.tsx`); this only
// opens it, in the room the Thread is shown in, the same way the Files
// drawer does:
//   - a file opens as a file tab (`openFileAsTab`, the drawer's plain click),
//     jumping to `:line[:col]` when the link had one;
//   - a folder opens the Files drawer and reveals it there.

//
// Thread file objects (attachments, 0.46.1) open through the same door:
// `openThreadAttachment` asks the room's server to resolve the stored
// workspace-relative path (`fs/resolve-links`: realpath, still inside the
// workspace) and then opens it exactly like a link.

import type { Room } from '@/stores/room'
import { usePanelsStore } from '@/stores/panels'
import { useFileTreeStore } from '@/stores/filetree'
import { requestFileLine, requestFileTreeReveal } from '@/lib/file-open-requests'
import type { FileLinkAnswer, FileLinkTransport } from '@/components/common/file-links'
import type { ThreadFileObject } from '@/components/SessionView/overlayThread'

/** Open a Thread file object: resolve its stored path against the
 *  Thread's workspace on the room's server, then open it like a link (a
 *  file tab, at its line). Resolves `false` when the server says it is
 *  no longer a file in the workspace (the chip then reads as gone). */
export async function openThreadAttachment(
  room: Pick<Room, 'scope' | 'tabs' | 'isPrimary'>,
  transport: FileLinkTransport,
  workspace: string,
  file: ThreadFileObject,
): Promise<boolean> {
  if (!file.path || file.state === 'hidden' || file.state === 'gone') return false
  const res = await transport(workspace, [file.path])
  const answer = (res.links ?? [])[0]
  if (!answer || !answer.linkable || !answer.path || answer.kind !== 'file') return false
  openThreadFileLink(room, answer as FileLinkAnswer & { path: string }, file.line)
  return true
}

/** "Find in Files" for a file object that moved: search the Files drawer
 *  for its name (the drawer's own name search). */
export function findThreadAttachment(room: Pick<Room, 'isPrimary'>, file: ThreadFileObject): void {
  if (!file.name) return
  useFileTreeStore.getState().setSearchQuery(file.name)
  if (room.isPrimary) usePanelsStore.getState().activateTab('files')
}

export function openThreadFileLink(
  room: Pick<Room, 'scope' | 'tabs' | 'isPrimary'>,
  link: FileLinkAnswer & { path: string },
  line?: number,
  col?: number,
): void {
  if (link.kind === 'directory') {
    requestFileTreeReveal(room.scope.hostKey, link.path)
    // The drawer panels are the window's; a pinned (Home) room's tree
    // reveals when its drawer is showing.
    if (room.isPrimary) usePanelsStore.getState().activateTab('files')
    return
  }
  if (line !== undefined) requestFileLine(room.scope.hostKey, link.path, line, col)
  room.tabs.getState().openFileAsTab(link.path)
}
