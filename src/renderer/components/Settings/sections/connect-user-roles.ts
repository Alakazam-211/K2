// Connect login roles for Settings → Server Access → Users.
//
// prd-remove-viewer-role-v1.md: the Viewer role is gone. A login is a
// trusted operator — Member, Admin or Owner. Limited access is what app
// passes are for. Accounts that were Viewers were disabled when the
// daemon updated and come back from GET /cli/users with
// `wasViewer: true`; the owner turns one back on with "Enable as Member"
// (POST /cli/users/set-disabled {disabled:false}), which also clears the
// flag. Nobody is upgraded without that click.

/** The login roles a daemon accepts. There is no `viewer`. */
export type K2Role = 'owner' | 'admin' | 'member'

/** Role picker options — lowest first. No Viewer option. */
export const ROLE_OPTIONS: ReadonlyArray<{ value: K2Role; label: string }> = [
  { value: 'member', label: 'Member' },
  { value: 'admin', label: 'Admin' },
  { value: 'owner', label: 'Owner' },
]

/** Parse a wire role. The retired `viewer` (an older server can still
 *  send it) and anything unknown are `null`, never coerced to Member. */
export function parseK2Role(raw: unknown): K2Role | null {
  return raw === 'owner' || raw === 'admin' || raw === 'member' ? raw : null
}

/** One row of GET /cli/users. `role` stays a string: an older server may
 *  still report `viewer`, which the UI shows as-is rather than guessing. */
export interface K2User {
  username: string
  createdAt?: string | null
  disabled: boolean
  role?: string
  /** True for an account disabled by the Viewer removal. */
  wasViewer?: boolean
}

/** Whether the row should offer "Enable as Member": a former Viewer that
 *  is still disabled. Once enabled the daemon clears `wasViewer`. */
export function needsEnableAsMember(u: Pick<K2User, 'wasViewer' | 'disabled'>): boolean {
  return u.wasViewer === true && u.disabled
}

/** Text under a former Viewer's name in the users list. */
export const WAS_VIEWER_NOTE =
  'Was a Viewer. The Viewer role was removed, so this login was turned off. Enable it as a Member to let them sign in again.'
