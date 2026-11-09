import React from 'react'
import { Checkbox } from '@/components/ui'

// Settings → Admin Access → per person "Can clone" (prd-clone-carries-
// threads-and-vault-v1 §4.2, CL23; D2: default ON). Owner-role rows are
// always allowed (shown on and locked). Only the Owner (owner token or an
// Owner login) changes it, through POST /cli/users/set-can-clone; the
// daemon's clone gate is the real check.

export interface CanCloneState {
  checked: boolean
  locked: boolean
  title: string
}

/** What the row shows for `rowRole` / `canClone` to a viewer of `myRole`. */
export function canCloneState(rowRole: string | undefined, canClone: boolean | undefined, myRole: string | null): CanCloneState {
  if (rowRole === 'owner') {
    return { checked: true, locked: true, title: 'Owners can always clone' }
  }
  // Absent on an older daemon: today's behaviour (allowed).
  const checked = canClone !== false
  if (myRole !== 'owner') {
    return { checked, locked: true, title: 'Only the owner changes who can clone' }
  }
  return { checked, locked: false, title: 'May clone agents from this server' }
}

export function CanCloneToggle({
  username,
  rowRole,
  canClone,
  myRole,
  onChange,
}: {
  username: string
  rowRole: string | undefined
  canClone: boolean | undefined
  myRole: string | null
  onChange: (next: boolean) => void
}): React.JSX.Element {
  const st = canCloneState(rowRole, canClone, myRole)
  return (
    <label
      className={`flex items-center gap-1.5 select-none no-drag ${st.locked ? 'opacity-60 cursor-not-allowed' : 'cursor-pointer'}`}
      title={st.title}
    >
      <Checkbox
        checked={st.checked}
        disabled={st.locked}
        aria-label={`${username} can clone`}
        data-testid={`can-clone-${username}`}
        onChange={(e) => onChange(e.target.checked)}
        style={st.locked ? { opacity: 1, cursor: 'not-allowed' } : undefined}
      />
      <span className="text-[10px] text-[var(--color-text-secondary)]">Can clone</span>
    </label>
  )
}

/** POST /cli/users/set-can-clone with optimistic flip + revert. `post`
 *  is the section's user-route poster (`userPost`). */
export async function setCanCloneOptimistic(args: {
  username: string
  next: boolean
  post: (suffix: string, body: unknown) => Promise<Response>
  apply: (username: string, canClone: boolean) => void
  errText: (res: Response) => Promise<string>
  onError: (msg: string) => void
  refresh: () => Promise<void>
}): Promise<void> {
  const { username, next, post, apply, errText, onError, refresh } = args
  apply(username, next)
  try {
    const res = await post('/set-can-clone', { username, canClone: next })
    if (!res.ok) {
      apply(username, !next)
      onError(await errText(res))
      return
    }
    await refresh()
  } catch (e) {
    apply(username, !next)
    onError(e instanceof Error ? e.message : 'Failed to change who can clone')
  }
}
