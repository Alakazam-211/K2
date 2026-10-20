// Clickable paths outside the main Thread (0.46.3): ticket chat, project
// chat and the Garden Thread render their messages through the same
// `ChatMessageBody`, which links paths only under a `FileLinkProvider`.
// This wraps one of those surfaces in the provider (and the file-chip
// actions), the same way `ThreadOverlayPane` does:
//   - the server named by `scope` decides which printed paths are files in
//     `workspace` (`fs/resolve-links`, daemon-first);
//   - a click opens the file in the window's own room (these pages are not
//     inside a RoomProvider), Option/Alt-click in the background.
// With no `workspace` (an unlinked ticket) the children render unchanged.

import { useMemo, useCallback, type JSX, type ReactNode } from 'react'
import { FileLinkProvider, scopeFileLinkTransport, type FileLinkOpen, type FileLinkTransport } from './file-links'
import { ThreadFileActionsProvider, type ThreadFileActions } from '@/components/Thread/ThreadFileObjects'
import { findThreadAttachment, openThreadAttachment, openThreadFileLink } from '@/lib/thread-file-link-open'
import type { ServerScope } from '@/kessel/server-scope'
import { primaryRoom, type Room } from '@/stores/room'

type OpenRoom = Pick<Room, 'scope' | 'tabs' | 'isPrimary'>

export function WorkspaceFileLinks({
  workspace,
  scope,
  room,
  transport: transportOverride,
  open: openOverride,
  children,
}: {
  /** A Thread address, workspace path or id the server can resolve. */
  workspace: string | null | undefined
  scope: ServerScope
  /** Where a click opens the file; defaults to the window's own room. */
  room?: OpenRoom
  /** Tests: stand-ins for the server resolve and the opener. */
  transport?: FileLinkTransport
  open?: FileLinkOpen
  children: ReactNode
}): JSX.Element {
  const scopeTransport = useMemo(() => scopeFileLinkTransport(scope), [scope])
  const transport = transportOverride ?? scopeTransport
  const roomFor = useCallback((): OpenRoom => room ?? primaryRoom(), [room])
  const defaultOpen = useCallback<FileLinkOpen>(
    (link, line, col, opts) => openThreadFileLink(roomFor(), link, line, col, opts),
    [roomFor],
  )
  const open = openOverride ?? defaultOpen
  const fileActions = useMemo<ThreadFileActions | null>(
    () =>
      workspace
        ? {
            open: (file, opts) => {
              void openThreadAttachment(roomFor(), transport, workspace, file, opts).catch((err: unknown) =>
                console.warn('[file-links] open attachment failed:', err),
              )
            },
            find: (file) => findThreadAttachment(roomFor(), file),
          }
        : null,
    [workspace, transport, roomFor],
  )
  if (!workspace || !fileActions) return <>{children}</>
  return (
    <ThreadFileActionsProvider value={fileActions}>
      <FileLinkProvider workspace={workspace} transport={transport} open={open}>
        {children}
      </FileLinkProvider>
    </ThreadFileActionsProvider>
  )
}
