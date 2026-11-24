// A project's chat as a Thread conversation (`project:<group id>`), on a
// server that keeps project chats on the Thread store
// (project-chat-thread-contract.ts). The Projects page draws the agent
// chat's Thread pane and floating header on it, so a project chat gets what
// every Thread has: fold, search, Reply, the message menu, delete your own
// line, typing, files, Jump to bottom.
//
// What differs from an agent chat:
//   - the header carries search, the point of contact's picture and name,
//     the mute bell (kind `project`) and the chat's close button: no view
//     menu, no Chatter or Heartbeats drawer, no harness move, and no
//     terminal drawer (a post always goes to the project, never into the
//     point of contact's PTY);
//   - the box has no "/" commands, and its draft is the project's;
//   - the strip over the box is the point of contact's own session at work
//     (the project's Thread has no activity of its own);
//   - a post's answer says whether the point of contact got the line, on
//     a note over the box;
//   - a login that may only read (no Connect role yet) gets no box;
//   - reading here moves the project's own read cursor.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { PrimaryRoom } from '@/components/Room/PrimaryRoom'
import { PageLiveContext } from '@/contexts/TabVisibilityContext'
import { useRoom, useRoomProjects } from '@/components/Room/RoomContext'
import { PinnedSessionBody } from '@/components/SessionView/AgentSessionChrome'
import { ThreadComposeExtrasContext, type ThreadComposeExtras } from '@/components/SessionView/threadComposeExtras'
import { ThreadFloatSearch } from '@/components/SessionView/ThreadFloatSearch'
import { ThreadViewIdentity } from '@/components/SessionView/ThreadViewIdentity'
import { useSessionViewTab } from '@/components/SessionView/useSessionViewTab'
import {
  THREAD_FLOATS_GRID,
  THREAD_FLOATS_GRID_ATTR,
  THREAD_FLOAT_CENTRE_CLASS,
  THREAD_FLOAT_GROUP_CLASS,
  ThreadFloatScrim,
  useThreadFloats,
} from '@/components/SessionView/threadFloats'
import { ThreadFloatMerge } from '@/components/SessionView/ThreadFloatMerge'
import { ThreadChatNote } from '@/components/Thread/ThreadChat'
import { useChatSessionTurn } from '@/components/Thread/sessionTurn'
import { MuteToggleButton } from '@/components/common/UnreadBadge'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { workspaceHandle } from '@/lib/home-address'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { canPostProjectChat, composerPlaceholder, deliveredLine, pocLabel, type ConnectRole } from './project-chat'
import ProjectChatPanel, { projectChatTarget } from './ProjectChatPanel'
import {
  projectConversationId,
  projectDraftId,
  projectPostOutcome,
  projectThreadAddr,
} from './project-chat-thread-contract'
import type { ProjectGroupShow } from './projects-api'

/** Room between the newest message and the list's foot (px), as a ticket's. */
export const PROJECT_REST_GAP_PX = 40

export interface ProjectThreadPaneProps {
  show: ProjectGroupShow
}

/** The Projects page's chat drawer: the Thread pane on `project:<id>` when
 *  the server keeps project chats on the Thread store (`threadOn`,
 *  projectChatThreadOn), else the old panel (keep-old-path: removed once
 *  every server has the feature). */
export function ProjectChatSurface({ show, width, threadOn }: { show: ProjectGroupShow; width: number; threadOn: boolean }): React.JSX.Element {
  if (!threadOn) return <ProjectChatPanel show={show} />
  return (
    <div
      data-drawer-panel=""
      className="relative flex-shrink-0 flex flex-col min-h-0 h-full border-l border-[var(--color-border)] bg-[var(--color-bg-surface)]"
      style={{ width }}
    >
      <ProjectThreadPane show={show} />
    </div>
  )
}

export function ProjectThreadPane(props: ProjectThreadPaneProps): React.JSX.Element {
  // On screen whenever it is drawn (the Projects page's chat, open): the
  // Thread follows the person's scroll and marks what they read.
  return (
    <PageLiveContext.Provider value={true}>
      <PrimaryRoom>
        <ProjectThreadBody key={props.show.id} {...props} />
      </PrimaryRoom>
    </PageLiveContext.Provider>
  )
}

function ProjectThreadBody({ show }: ProjectThreadPaneProps): React.JSX.Element {
  const room = useRoom()
  const groupId = show.id
  const addr = projectThreadAddr(groupId)
  const conversationId = projectConversationId(groupId)
  // Always Thread view; no project id, so no workspace chat's view state.
  const view = useSessionViewTab(conversationId, room.scope, null)

  // Connect role decides the box (not window-mode): null while loading →
  // read only (fail closed), as the old panel.
  const [connectRole, setConnectRole] = useState<ConnectRole | null>(null)
  useEffect(() => {
    let cancelled = false
    void fetchViewerRole().then((role) => {
      if (!cancelled) setConnectRole(role)
    })
    return () => {
      cancelled = true
    }
  }, [])
  const readOnly = !canPostProjectChat(connectRole)

  // The point of contact: its picture and name in the header, its folder
  // for the box's files (they are kept in its workspace).
  const pocId = show.pocWorkspaceId
  const poc = pocLabel(show.members, pocId)
  const pocProject = useRoomProjects(useCallback((projects) => (pocId ? (projects.find((p) => p.id === pocId) ?? null) : null), [pocId]))
  const pocHandle = pocProject ? workspaceHandle(pocProject) : null
  const pocPath = pocProject?.path ?? undefined
  const turn = useChatSessionTurn(room.scope, projectChatTarget(pocId))

  // The last post's delivery line, over the box until the next post.
  const [delivery, setDelivery] = useState<string | null>(null)
  const onPosted = useCallback(
    (resp: Record<string, unknown>): void => {
      const outcome = projectPostOutcome(resp)
      if (!outcome || outcome.delivered === null) {
        setDelivery(null)
        return
      }
      setDelivery(deliveredLine(outcome.delivered, outcome.deliveryReason, poc))
    },
    [poc],
  )
  const above = useMemo(
    () =>
      delivery ? (
        <ThreadChatNote testId="project-thread-delivery">
          <span data-project-chat-delivery="">{delivery}</span>
        </ThreadChatNote>
      ) : null,
    [delivery],
  )
  const readAs = useMemo(() => ({ kind: 'project' as const, id: groupId }), [groupId])
  const extras = useMemo<ThreadComposeExtras>(
    () => ({
      above,
      placeholder: composerPlaceholder(show.members, pocId),
      slash: false,
      turn,
      onPosted,
      restGapPx: PROJECT_REST_GAP_PX,
      footInFlow: true,
      headerBlur: 'heavy',
      readOnly,
      readAs,
    }),
    [above, show.members, pocId, turn, onPosted, readOnly, readAs],
  )

  return (
    <div
      className="relative flex-1 min-h-0 flex flex-col"
      data-testid="project-thread-pane"
      data-project-thread-pane=""
      data-project-thread-addr={addr}
      data-project-thread-readonly={readOnly ? '' : undefined}
    >
      <ProjectThreadHeader groupId={groupId} name={poc ?? show.name} addr={pocHandle ?? addr} />
      <ThreadComposeExtrasContext.Provider value={extras}>
        <PinnedSessionBody
          viewTab="thread"
          splitLeft={view.splitLeft}
          splitRight={view.splitRight}
          addr={addr}
          conversationId={conversationId}
          agentName={pocId ?? ''}
          draftId={projectDraftId(groupId)}
          displayName={poc ?? show.name}
          terminalRevealed={false}
          terminalRevealKey={null}
          projectPath={pocPath}
        >
          {null}
        </PinnedSessionBody>
      </ThreadComposeExtrasContext.Provider>
    </div>
  )
}

/** The floating header over the project's Thread: search, the point of
 *  contact's picture and name, the mute bell and the chat's close. */
function ProjectThreadHeader({ groupId, name, addr }: { groupId: string; name: string; addr: string }): React.JSX.Element {
  const floatsRef = useThreadFloats(true)
  return (
    <div
      ref={floatsRef}
      className={THREAD_FLOATS_GRID}
      data-testid="project-thread-header"
      data-project-thread-header=""
      data-session-header=""
      data-thread-floats=""
      {...{ [THREAD_FLOATS_GRID_ATTR]: '' }}
    >
      <ThreadFloatScrim />
      <ThreadFloatMerge />
      <div className={THREAD_FLOAT_GROUP_CLASS}>
        <ThreadFloatSearch />
      </div>
      <div className={THREAD_FLOAT_CENTRE_CLASS}>
        <ThreadViewIdentity name={name} addr={addr} bar />
      </div>
      <div className={`${THREAD_FLOAT_GROUP_CLASS} justify-end`}>
        <MuteToggleButton kind="project" id={groupId} noun="project chat" />
        <button
          type="button"
          className="k2-focus no-drag flex items-center justify-center w-6 h-6 flex-shrink-0 rounded-[var(--radius-field)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-row-hover)] transition-colors"
          data-k2-chamfer="control"
          aria-label="Close the chat"
          title="Close the chat"
          onClick={() => useProjectGroupsStore.getState().setChatCollapsed(true)}
          data-project-chat-close=""
        >
          <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" className="w-3.5 h-3.5 block">
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </button>
      </div>
    </div>
  )
}
