// Sew access on the Access pages (S32 K28 prep, existing routes only).
//
// - SewPersonAccess (User Access): a person's Sew access — No access / Sew
//   member / Sew guest — through their `chat` grant (`/cli/skin/grants*`),
//   with the grant hook's sync state and a Retry for a blocked row. A Sew
//   grant creates the person's Sew account; what they see is set inside Sew.
// - SewAdminAccessState (Admin Access): READ-ONLY. A Connect login gets a
//   Sew account only from its own `connect` chat grant, never from its role
//   (Rosson rev 147: Connect owners/admins are server control, not Sew
//   members by role). Shown only where such a grant exists; no controls.
//
// Both read `GET /cli/sew/status` through the shared 30 s cache and hide
// themselves when the daemon says Sew isn't supported here, or the read
// fails (the page works as before). Never `fetchProjects()`.

import React, { useEffect, useState } from 'react'
import { Button, Tag } from '@/components/ui'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import { useWindowModeStore } from '@/stores/window-mode'
import { SETTING_FIELD_WIDTH, SettingDropdown, SettingRow, SettingsGroup } from '../controls/SettingControls'
import {
  blockedHookRow,
  fetchChatGrants,
  grantPersonSew,
  retryHookRow,
  revokeSewGrant,
  sewErrorMessage,
  sewStatusCached,
  updateSewGrantScope,
  type ChatGrant,
  type SewStatus,
} from './sew-api'

const muted = 'text-[var(--color-text-muted)]'
const PENDING_REFRESH_MS = 5000

export const SEW_ACCESS_TOOLTIP = "Giving chat access makes the person a chat account on this server. Which channels they see is set in the chat itself."

type Level = 'none' | 'member' | 'guest'

function levelOf(grant: ChatGrant | null): Level {
  if (!grant) return 'none'
  return grant.scope === 'guest' ? 'guest' : 'member'
}

/** The shared status read; `null` while loading, `false` when hidden. */
function useSewStatus(): SewStatus | null | false {
  const [s, setS] = useState<SewStatus | null | false>(null)
  useEffect(() => {
    let live = true
    sewStatusCached().then(
      (v) => live && setS(v.supported ? v : false),
      () => live && setS(false),
    )
    return () => {
      live = false
    }
  }, [])
  return s
}

export function HookStatePill({
  grant,
  onRetry,
}: {
  grant: Pick<ChatGrant, 'hookState' | 'hookError'>
  onRetry?: () => void
}): React.JSX.Element | null {
  switch (grant.hookState) {
    case 'pending':
      return (
        <Tag tone="warn" data-testid="sew-hook-pill" title={grant.hookError ?? 'K2 keeps telling Sew'}>
          Pending
        </Tag>
      )
    case 'blocked':
      return (
        <span className="flex items-center gap-1.5">
          <Tag tone="danger" data-testid="sew-hook-pill" title={grant.hookError ?? ''}>
            Failed
          </Tag>
          {onRetry ? (
            <Button onClick={onRetry} aria-label="Retry Sew sync">
              Retry
            </Button>
          ) : null}
        </span>
      )
    case 'held':
      return (
        <Tag tone="muted" data-testid="sew-hook-pill" title={grant.hookError ?? 'Sew holds this member (banned in a Space)'}>
          Held
        </Tag>
      )
    case 'synced':
      return (
        <Tag tone="ok" data-testid="sew-hook-pill">
          Active
        </Tag>
      )
    default:
      return null
  }
}

/** User Access → a person's Sew access. `grant` is their chat grant (from
 *  the page's own `skin/grants` read, with `hookState`); `onChanged`
 *  refetches it. */
export function SewPersonAccess({
  principalId,
  username,
  grant,
  onChanged,
}: {
  principalId: string | null
  username: string
  grant: ChatGrant | null
  onChanged: () => Promise<void>
}): React.JSX.Element | null {
  const status = useSewStatus()
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const [optimistic, setOptimistic] = useState<Level | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const pending = grant?.hookState === 'pending'

  // View refresh while K2's worker is still telling Sew (the worker
  // retries on its own, headless; this only repaints).
  useEffect(() => {
    if (!pending) return
    const t = window.setInterval(() => void onChanged(), PENDING_REFRESH_MS)
    return () => window.clearInterval(t)
  }, [pending, onChanged])

  if (status === false || status === null) return null
  const installed = !(status.state === 'not-installed' || status.state === 'removed')
  const targetOn = status.target?.enabled !== false
  const value = optimistic ?? levelOf(grant)
  const disabled = busy || viewerReadOnly || !principalId || !installed

  const change = async (next: Level): Promise<void> => {
    if (!principalId || next === value) return
    const prev = value
    if (grant && next === 'none') {
      const ok = await useConfirmDialogStore.getState().confirm({
        title: `Remove ${username}'s chat and voice access?`,
        message: `Removes ${username}'s chat account. Their messages stay.`,
        confirmLabel: 'Remove',
        destructive: true,
      })
      if (!ok) return
    } else if (grant && prev === 'member' && next === 'guest') {
      const ok = await useConfirmDialogStore.getState().confirm({
        title: `Make ${username} a chat guest?`,
        message: 'Guests see only channels they are added to.',
        confirmLabel: 'Make guest',
      })
      if (!ok) return
    }
    setOptimistic(next)
    setBusy(true)
    setError(null)
    try {
      if (!grant) {
        await grantPersonSew(principalId, next === 'guest' ? 'guest' : 'member')
      } else if (next === 'none') {
        const r = await revokeSewGrant(grant.id)
        const n = Array.isArray(r?.cascaded) ? r.cascaded.length : 0
        if (n > 0) useToastStore.getState().addToast(`Also removed chat and voice access for ${n} assistant agent${n === 1 ? '' : 's'}.`, 'info')
      } else {
        await updateSewGrantScope(grant.id, next === 'guest' ? 'guest' : 'member')
      }
      await onChanged()
    } catch (e) {
      setError(sewErrorMessage(e))
    } finally {
      setOptimistic(null)
      setBusy(false)
    }
  }

  const retry = async (): Promise<void> => {
    if (!principalId) return
    setError(null)
    try {
      const id = await blockedHookRow(`k2:principal:${principalId}`)
      if (!id) throw new Error('No failed sync row for this person any more.')
      await retryHookRow(id)
      await onChanged()
    } catch (e) {
      setError(sewErrorMessage(e))
    }
  }

  const hint = !principalId
    ? 'K2 on the server needs an update before this person can use chat.'
    : !installed
      ? 'Chat / Voice Hosting isn’t set up on this server yet.'
      : !targetOn && grant
        ? 'Chat / Voice Hosting is turned off, so this can’t change now. You can still remove it.'
        : 'Members join the chat on this server. Guests see only the channels they’re added to.'
  return (
    <SettingsGroup title="Chat and voice" hint={SEW_ACCESS_TOOLTIP} card testId="sew-person-access">
      <SettingRow
        label="Chat access"
        hint={hint}
        below={
          error ? (
            <p role="alert" className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">
              {error}
            </p>
          ) : undefined
        }
      >
        {grant ? <HookStatePill grant={grant} onRetry={() => void retry()} /> : null}
        {installed && (targetOn || !grant) ? (
          <div className={SETTING_FIELD_WIDTH}>
            <SettingDropdown
              ariaLabel={`Chat and voice access for ${username}`}
              value={value}
              fullWidth
              disabled={disabled || (!targetOn && !grant)}
              options={[
                { value: 'none', label: 'No access' },
                { value: 'member', label: 'Member', disabled: !targetOn },
                { value: 'guest', label: 'Guest', disabled: !targetOn },
              ]}
              onChange={(v) => void change(v as Level)}
            />
          </div>
        ) : installed ? (
          <>
            <span className="type-row text-[var(--color-text-secondary)]">{value === 'guest' ? 'Guest' : 'Member'}</span>
            <Button disabled={disabled} onClick={() => void change('none')}>
              Remove
            </Button>
          </>
        ) : (
          <Button onClick={() => useSettingsStore.setState({ activeSection: 'sew' })}>Set up chat</Button>
        )}
      </SettingRow>
    </SettingsGroup>
  )
}

/** Admin Access → Sew state for Connect logins, read only, only where a
 *  `connect` chat grant exists. `users` rows carry the login's stable `id`
 *  (the grant's subjectId); a row without one can't be matched and the
 *  grant shows by id. */
export function SewAdminAccessState({ users }: { users: { id?: string; username: string }[] }): React.JSX.Element | null {
  const status = useSewStatus()
  const [grants, setGrants] = useState<ChatGrant[] | null>(null)
  useEffect(() => {
    if (!status) return
    let live = true
    fetchChatGrants().then(
      (g) => live && setGrants(g.filter((x) => x.subjectKind === 'connect')),
      () => live && setGrants([]),
    )
    return () => {
      live = false
    }
  }, [status])
  if (!status || !grants || grants.length === 0) return null
  const nameOf = (id: string): string => users.find((u) => u.id === id)?.username ?? id
  return (
    <div className="space-y-1 border border-[var(--color-border)] p-2" data-testid="sew-admin-access">
      <div className="type-name text-[var(--color-text-primary)]">Chat / Voice Hosting (read only)</div>
      <p className={`type-hint ${muted}`}>
        A login uses chat and voice only through its own chat grant, never because of its role here. Admin Access never adds anyone to an App&apos;s
        Spaces.
      </p>
      <ul className="space-y-0.5">
        {grants.map((g) => (
          <li key={g.id} className="flex items-center gap-2 type-hint" data-testid={`sew-admin-row-${nameOf(g.subjectId)}`}>
            <span className="font-mono text-[var(--color-text-primary)]">{nameOf(g.subjectId)}</span>
            <span className="text-[var(--color-text-secondary)]">
              {g.scope === 'guest' ? 'Chat guest' : 'Chat member'}
              {g.enabled ? '' : ' (off)'}
            </span>
            <HookStatePill grant={g} />
          </li>
        ))}
      </ul>
    </div>
  )
}
