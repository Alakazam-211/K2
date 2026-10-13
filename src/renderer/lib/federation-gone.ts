// 0159: a federated connection whose workspace left its server ("gone").
// Pure helpers, kept apart from `federation.ts` so row code and tests needn't
// load the store graph.

/** The line a gone connection shows (desktop Settings + Workspace panel). */
export function remoteConnectionGoneNote(host: string): string {
  return `Workspace no longer exists on ${host || 'that server'}. Re-add to reconnect.`
}

/** Only an owner or admin may re-add a gone connection; the daemon refuses
 *  anyone else (`connections add` checks the actor). `null` = the role is
 *  unknown (whoami failed): no button, the daemon still decides. */
export function canReAddGoneConnection(role: string | null): boolean {
  return role === 'owner' || role === 'admin'
}
