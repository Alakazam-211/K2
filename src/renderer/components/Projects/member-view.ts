// 0302: "Opens in", per agent. The view one agent's tile opens in on a
// project's dashboards, for everyone: its own (`ownDefaultView`), else the
// project's (`defaultView` on the project). Daemon-owned
// (`set-member-default-view`), so every client and the phone agree; a
// person's own switch on a tile stays theirs (tile-view.ts).
//
// The nav's right-click menu (a submenu) and the settings sheet's agent rows
// (a menu button) both read and set it through `useMemberViews`. Heartbeats
// is never a shared view: it stays a personal switch.

import { useCallback, useEffect, useState } from 'react'
import { httpCodeOf } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'
import {
  daemonErrorInfo,
  memberDefaultView,
  memberOwnDefaultView,
  projectDefaultView,
  setProjectGroupMemberDefaultView,
  type ProjectDefaultView,
  type ProjectGroup,
  type ProjectGroupMemberInfo,
} from './projects-api'
import { PRODUCT_NAME } from '@/lib/brand'

export const MEMBER_VIEW_NAME: Record<ProjectDefaultView, string> = {
  terminal: 'Terminal',
  thread: 'Thread',
  chatter: 'Chatter',
}

/** One agent's view: the one its tile opens in, and its own (null = it
 *  follows the project's). */
export interface MemberView {
  view: ProjectDefaultView
  own: ProjectDefaultView | null
}

/** "Project default (Thread)": the choice that clears an agent's own view. */
export function projectDefaultLabel(projectView: ProjectDefaultView): string {
  return `Project default (${MEMBER_VIEW_NAME[projectView]})`
}

/** The current choice in a few words: "Terminal" for the agent's own,
 *  "Thread (project default)" when it follows the project. */
export function memberViewSummary(v: MemberView): string {
  return v.own === null ? `${MEMBER_VIEW_NAME[v.view]} (project default)` : MEMBER_VIEW_NAME[v.view]
}

/** What a saved change says. */
export function memberViewSavedText(name: string, next: ProjectDefaultView | null, projectView: ProjectDefaultView): string {
  return next === null
    ? `${name} opens in the project default (${MEMBER_VIEW_NAME[projectView]}) again.`
    : `${name} now opens in ${MEMBER_VIEW_NAME[next]} for everyone.`
}

/** Why a change was refused. A refusal by role or permission, or an agent
 *  that left the project, is one plain sentence (trying again won't help);
 *  anything else is the server's reason, with Try again. */
export type MemberViewError = { plain: string } | { reason: string }

export function memberViewError(err: unknown, name: string): MemberViewError {
  const code = httpCodeOf(err) ?? daemonErrorInfo(err).code
  if (code === 'forbidden') return { plain: 'Couldn’t change it. Only the owner or an admin can.' }
  if (code === 'agent_permission_off') return { plain: `Couldn’t change it. This agent needs Manage ${PRODUCT_NAME} projects on.` }
  if (code === 'not_a_member') return { plain: `Couldn’t change it. ${name} isn’t in this project any more.` }
  const { hint } = daemonErrorInfo(err)
  return { reason: hint ?? (err instanceof Error ? err.message : String(err)) }
}

/** Every member's view, with a change shown at once (optimistic) until the
 *  next `show` read carries it. `set` resolves with the daemon's answer or
 *  rolls back and rethrows. `scope`: the server the project is on. */
export function useMemberViews(
  project: Pick<ProjectGroup, 'id' | 'defaultView'> & { members: ProjectGroupMemberInfo[] },
  scope: ServerScope,
): {
  viewOf: (m: ProjectGroupMemberInfo) => MemberView
  busy: string | null
  set: (m: ProjectGroupMemberInfo, next: ProjectDefaultView | null) => Promise<void>
} {
  const [over, setOver] = useState<Record<string, MemberView>>({})
  const [busy, setBusy] = useState<string | null>(null)
  // A fresh read of the project is the truth again.
  useEffect(() => setOver({}), [project.members, project.id])

  const viewOf = useCallback(
    (m: ProjectGroupMemberInfo): MemberView =>
      over[m.workspaceId] ?? { view: memberDefaultView(m, project), own: memberOwnDefaultView(m) },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [over, project.defaultView],
  )

  const set = useCallback(
    async (m: ProjectGroupMemberInfo, next: ProjectDefaultView | null): Promise<void> => {
      const id = m.workspaceId
      const projectView = projectDefaultView(project)
      setBusy(id)
      setOver((o) => ({ ...o, [id]: { view: next ?? projectView, own: next } }))
      try {
        const r = await setProjectGroupMemberDefaultView(scope, project.id, id, next)
        if (r && typeof r.defaultView === 'string') {
          setOver((o) => ({ ...o, [id]: { view: r.defaultView, own: r.ownDefaultView ?? null } }))
        }
      } catch (err) {
        setOver((o) => {
          const rest = { ...o }
          delete rest[id]
          return rest
        })
        throw err
      } finally {
        setBusy((b) => (b === id ? null : b))
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [project.id, project.defaultView, scope],
  )

  return { viewOf, busy, set }
}
