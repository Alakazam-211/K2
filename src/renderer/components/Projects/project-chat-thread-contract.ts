// A project's chat as a Thread conversation (`project:<group id>`): every
// assumption the project pane makes about the daemon, in one place
// (project-chat-thread-v1).
//
// On a server that reports PROJECT_CHAT_THREAD_FEATURE, a project's chat is
// a Thread conversation of its own. The Projects page then draws the agent
// chat's Thread pane on it (ProjectThreadPane) instead of its own panel. An
// older server keeps the old panel (ProjectChatPanel).
//
// What the pane relies on:
//   - the address and the conversation id are both `project:<group id>`;
//     the Thread routes (read, deltas, attachments, delete, file links) and
//     the live socket take it as is (logins only);
//   - a post is a normal Thread post (`POST /cli/thread/post` with
//     `addr: 'project:<id>'`, a `clientMsgId`, files as `[{path}]` relative
//     to the point of contact's workspace). The daemon runs the project's
//     rules: the line reaches the point of contact as `[project:<name>]`;
//   - the post's answer carries `project: {groupId, groupName, delivered,
//     deliveryReason}` (projectPostOutcome); a person's post answers
//     `deliveryReason: 'queued'` (delivered after the answer);
//   - read marks and mutes are the project's (kind `project`, by group id),
//     the cursor every project surface reads;
//   - a project chat takes messages and files only: the card routes answer
//     `400 project_messages_only`;
//   - the point of contact's work shows in its own session, not in the
//     project's Thread activity.

import type { ServerScope } from '@/kessel/server-scope'

/** The server feature that turns the Thread pane on for project chats. */
export const PROJECT_CHAT_THREAD_FEATURE = 'project-chat-thread-v1'

const PROJECT_PREFIX = 'project:'

/** Does this server keep project chats on the Thread store? */
export function projectChatThreadOn(scope: Pick<ServerScope, 'serverSupports'>): boolean {
  return scope.serverSupports(PROJECT_CHAT_THREAD_FEATURE)
}

/** The Thread address a project's chat is posted to and read from. */
export function projectThreadAddr(groupId: string): string {
  return `${PROJECT_PREFIX}${groupId}`
}

/** The project's Thread conversation id (the live socket). */
export function projectConversationId(groupId: string): string {
  return `${PROJECT_PREFIX}${groupId}`
}

/** The message box's draft owner: the project, never its point of
 *  contact's own chat (whose draft is keyed by its workspace id). */
export function projectDraftId(groupId: string): string {
  return `${PROJECT_PREFIX}${groupId}`
}

/** What a post on the project did (the post's `project`). */
export interface ProjectPostOutcome {
  /** Null: not known (an older answer, a send-once repeat). */
  delivered: boolean | null
  deliveryReason: string | null
}

/** The `project` part of a post's answer; null when it has none. */
export function projectPostOutcome(resp: Record<string, unknown> | null | undefined): ProjectPostOutcome | null {
  const p = resp?.project
  if (!p || typeof p !== 'object' || Array.isArray(p)) return null
  const o = p as Record<string, unknown>
  return {
    delivered: typeof o.delivered === 'boolean' ? o.delivered : null,
    deliveryReason: typeof o.deliveryReason === 'string' && o.deliveryReason ? o.deliveryReason : null,
  }
}
