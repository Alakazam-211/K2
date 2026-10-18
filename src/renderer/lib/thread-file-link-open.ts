// What a Thread file link does when clicked (Thread file links, Rosson
// 2026-10-18). The daemon already said the path is a file or folder inside
// the Thread's workspace (`components/common/file-links.tsx`); this only
// opens it, in the room the Thread is shown in, the same way the Files
// drawer does:
//   - a file opens as a file tab (`openFileAsTab`, the drawer's plain click),
//     jumping to `:line[:col]` when the link had one;
//   - a folder opens the Files drawer and reveals it there.

import type { Room } from '@/stores/room'
import { usePanelsStore } from '@/stores/panels'
import { requestFileLine, requestFileTreeReveal } from '@/lib/file-open-requests'
import type { FileLinkAnswer } from '@/components/common/file-links'

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
