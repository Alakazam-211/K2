// Shared inbox access panel — the ONE permission surface for every inbox,
// hosted OR linked (GH #28: "create is hosting-specific, access is not").
// Given a unified `Inbox`, it renders the **Primary** workspace (with its
// own level), the **grants** list (each with level / Make-Primary / Remove),
// and an **Add access** row. It drives the unified /cli/mail/access/* routes:
// set-level, set-primary (transfer), grant, revoke.
//
// LEVEL-AWARE (BINDING): the Send level is offered whenever the inbox's
// `maxLevel === 'send'` — true for hosted inboxes AND, as of the linked-send
// contract, for linked inboxes too (sending from a linked inbox goes out as
// your real external account, ungated for now). Driving off `maxLevel` keeps
// this correct for both sources and future-proof. Updates are optimistic
// (patch the parent's inbox row, revert on error); the daemon's structured
// `{code, hint}` errors (403 not-primary, not_found) surface inline. This
// panel NEVER shows credentials.

import React, { useCallback, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { Toggle } from '@/components/ui'
import { SettingDropdown } from '../controls/SettingControls'
import {
  grantInboxAccess,
  mailErrorMessage,
  revokeInboxAccess,
  setInboxLevel,
  setInboxManage,
  setInboxPrimary,
  type Inbox,
  type InboxLevel,
  type InboxParticipant,
} from './email-api'

const LEVEL_LABEL: Record<InboxLevel, string> = {
  read: 'Read-only',
  draft: 'Read + Draft',
  send: 'Read + Draft + Send',
}

/** Level options for a dropdown — Read / Draft always; Send whenever the
 *  inbox's `maxLevel` permits it (hosted always; linked now that linked-send
 *  is allowed). Driving off `maxLevel` keeps this correct for both sources. */
function levelOptions(maxLevel: Inbox['maxLevel']): { value: InboxLevel; label: string }[] {
  const opts: { value: InboxLevel; label: string }[] = [
    { value: 'read', label: LEVEL_LABEL.read },
    { value: 'draft', label: LEVEL_LABEL.draft },
  ]
  if (maxLevel === 'send') opts.push({ value: 'send', label: LEVEL_LABEL.send })
  return opts
}

function SectionTitle({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
      {children}
    </h3>
  )
}

/** The per-workspace mailbox-management capabilities: "Allow inbox management"
 *  and, nested beneath it, "Allow delete". Delete requires management, so the
 *  nested toggle only appears while management is ON, and turning management
 *  OFF forces delete OFF in the same change — the callback NEVER emits the
 *  invalid `canDelete && !canManage` combo the daemon rejects. */
function ManageToggles({
  participant,
  disabled,
  onChange,
}: {
  participant: InboxParticipant
  disabled: boolean
  /** Emits the resulting flags; delete is already forced false when manage
   *  goes off, so this is always a server-valid pair. */
  onChange: (canManage: boolean, canDelete: boolean) => void
}): React.JSX.Element {
  const { canManage, canDelete } = participant
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between gap-2">
        <span className="text-[11px] text-[var(--color-text-secondary)]">
          Allow inbox management
        </span>
        <Toggle
          checked={canManage}
          disabled={disabled}
          // Manage off → delete off, in one server-valid change.
          onChange={(next) => onChange(next, next ? canDelete : false)}
          aria-label="Allow inbox management"
        />
      </div>
      {canManage && (
        <div className="flex items-center justify-between gap-2 pl-5">
          <span className="text-[11px] text-[var(--color-text-secondary)]">Allow delete</span>
          <Toggle
            checked={canDelete}
            disabled={disabled}
            onChange={(next) => onChange(true, next)}
            aria-label="Allow delete"
          />
        </div>
      )}
    </div>
  )
}

export function InboxAccessPanel({
  inbox,
  canMutate,
  patchInbox,
}: {
  inbox: Inbox
  canMutate: boolean
  /** Optimistically patch this inbox's row in the parent list (primary +
   *  grants) — no refetch (no fetch-in-render). */
  patchInbox: (address: string, patch: (i: Inbox) => Inbox) => void
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const [error, setError] = useState<string | null>(null)
  // projectId in flight, or 'add', or `primary:<id>` for a transfer.
  const [pending, setPending] = useState<string | null>(null)
  const [newProject, setNewProject] = useState('')
  const [newLevel, setNewLevel] = useState<InboxLevel>('read')

  const options = useMemo(() => levelOptions(inbox.maxLevel), [inbox.maxLevel])
  const canSend = inbox.maxLevel === 'send'
  const primaryId = inbox.primary.projectId
  const grantedIds = useMemo(() => new Set(inbox.grants.map((g) => g.projectId)), [inbox.grants])
  const projectName = useCallback(
    (id: string): string => projects.find((p) => p.id === id)?.name ?? id,
    [projects],
  )

  // Change the Primary's OWN level (project = primary). Optimistic.
  const changePrimaryLevel = useCallback(
    async (level: InboxLevel): Promise<void> => {
      setError(null)
      setPending(primaryId)
      const before = inbox.primary
      patchInbox(inbox.address, (i) => ({ ...i, primary: { ...i.primary, level } }))
      try {
        await setInboxLevel({ address: inbox.address, project: primaryId, level })
      } catch (e) {
        patchInbox(inbox.address, (i) => ({ ...i, primary: before }))
        setError(mailErrorMessage(e))
      } finally {
        setPending(null)
      }
    },
    [inbox.address, inbox.primary, primaryId, patchInbox],
  )

  // Change a grant's level. Optimistic.
  const changeGrantLevel = useCallback(
    async (projectId: string, level: InboxLevel): Promise<void> => {
      setError(null)
      setPending(projectId)
      const before = inbox.grants
      patchInbox(inbox.address, (i) => ({
        ...i,
        grants: i.grants.map((g) => (g.projectId === projectId ? { ...g, level } : g)),
      }))
      try {
        await setInboxLevel({ address: inbox.address, project: projectId, level })
      } catch (e) {
        patchInbox(inbox.address, (i) => ({ ...i, grants: before }))
        setError(mailErrorMessage(e))
      } finally {
        setPending(null)
      }
    },
    [inbox.address, inbox.grants, patchInbox],
  )

  // Change a participant's management capabilities (Primary OR a grant, keyed
  // by projectId). Enforces canDelete⇒canManage client-side so the payload is
  // never the combo the daemon rejects. Optimistic.
  const changeManage = useCallback(
    async (projectId: string, canManage: boolean, canDelete: boolean): Promise<void> => {
      const nextDelete = canManage ? canDelete : false
      setError(null)
      setPending(`manage:${projectId}`)
      const beforePrimary = inbox.primary
      const beforeGrants = inbox.grants
      const apply = (p: InboxParticipant): InboxParticipant =>
        p.projectId === projectId ? { ...p, canManage, canDelete: nextDelete } : p
      patchInbox(inbox.address, (i) => ({
        ...i,
        primary: apply(i.primary),
        grants: i.grants.map(apply),
      }))
      try {
        await setInboxManage({ address: inbox.address, project: projectId, canManage, canDelete: nextDelete })
      } catch (e) {
        patchInbox(inbox.address, (i) => ({ ...i, primary: beforePrimary, grants: beforeGrants }))
        setError(mailErrorMessage(e))
      } finally {
        setPending(null)
      }
    },
    [inbox.address, inbox.primary, inbox.grants, patchInbox],
  )

  const doRevoke = useCallback(
    async (projectId: string): Promise<void> => {
      setError(null)
      setPending(projectId)
      const before = inbox.grants
      patchInbox(inbox.address, (i) => ({
        ...i,
        grants: i.grants.filter((g) => g.projectId !== projectId),
      }))
      try {
        await revokeInboxAccess({ address: inbox.address, project: projectId })
      } catch (e) {
        patchInbox(inbox.address, (i) => ({ ...i, grants: before }))
        setError(mailErrorMessage(e))
      } finally {
        setPending(null)
      }
    },
    [inbox.address, inbox.grants, patchInbox],
  )

  // Transfer Primary to a granted workspace: the target becomes Primary
  // (keeps its level); the old Primary demotes to a grant (keeps its level).
  const doMakePrimary = useCallback(
    async (projectId: string): Promise<void> => {
      setError(null)
      setPending(`primary:${projectId}`)
      const beforePrimary = inbox.primary
      const beforeGrants = inbox.grants
      patchInbox(inbox.address, (i) => {
        const target = i.grants.find((g) => g.projectId === projectId)
        if (!target) return i
        const grants = i.grants
          .filter((g) => g.projectId !== projectId)
          .concat([{ ...i.primary }])
        return {
          ...i,
          primary: { ...target },
          grants,
        }
      })
      try {
        await setInboxPrimary({ address: inbox.address, project: projectId })
      } catch (e) {
        patchInbox(inbox.address, (i) => ({ ...i, primary: beforePrimary, grants: beforeGrants }))
        setError(mailErrorMessage(e))
      } finally {
        setPending(null)
      }
    },
    [inbox.address, inbox.primary, inbox.grants, patchInbox],
  )

  // Grant OR upsert a new workspace at the chosen level.
  const doAdd = useCallback(async (): Promise<void> => {
    if (!newProject) return
    const projectId = newProject
    const level = newLevel
    setError(null)
    setPending('add')
    const before = inbox.grants
    patchInbox(inbox.address, (i) => {
      const exists = i.grants.some((g) => g.projectId === projectId)
      const grants = exists
        ? i.grants.map((g) => (g.projectId === projectId ? { ...g, level } : g))
        : [...i.grants, { projectId, workspace: projectName(projectId), level, canManage: false, canDelete: false }]
      return { ...i, grants }
    })
    try {
      await grantInboxAccess({ address: inbox.address, project: projectId, level })
      setNewProject('')
      setNewLevel('read')
    } catch (e) {
      patchInbox(inbox.address, (i) => ({ ...i, grants: before }))
      setError(mailErrorMessage(e))
    } finally {
      setPending(null)
    }
  }, [newProject, newLevel, inbox.address, inbox.grants, patchInbox, projectName])

  // Candidates for a new grant: every workspace that isn't the Primary and
  // doesn't already hold a grant.
  const addable = useMemo(
    () => projects.filter((p) => p.id !== primaryId && !grantedIds.has(p.id)),
    [projects, primaryId, grantedIds],
  )
  const busy = pending !== null
  const disabledCls = !canMutate || busy ? 'opacity-50 pointer-events-none' : undefined

  return (
    <div className="space-y-2">
      <SectionTitle>Workspace access</SectionTitle>
      <p className="text-[10px] text-[var(--color-text-muted)]">
        The <strong>Primary</strong> workspace manages this inbox and its access list. Grant other
        workspaces <strong>Read-only</strong>{canSend ? ', ' : ' or '}
        <strong>Read + Draft</strong>
        {canSend && (
          <>
            {' '}or <strong>Read + Send</strong> — granting Send lets that workspace's agents send
            email as this account
            {inbox.source === 'linked' ? ' (from your real external account)' : ''}. Sending is
            immediate for now — there is no approval step yet.
          </>
        )}{' '}
        Agents use the inbox via <span className="font-mono">k2 mail</span>.
      </p>
      <p className="text-[10px] text-[var(--color-text-muted)]">
        <strong>Inbox management</strong> lets a workspace move and organize mail into folders;{' '}
        <strong>Allow delete</strong> lets it move messages to Trash (recoverable) — not permanent
        removal. Delete requires management.
      </p>

      <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
        {/* Primary row — workspace + its own level (editable), no remove. */}
        <div className="px-3 py-2 space-y-2">
          <div className="flex items-center gap-2">
            <span className="text-xs text-[var(--color-text-primary)] truncate flex-1 min-w-0">
              {inbox.primary.workspace ?? inbox.primary.projectId}
            </span>
            <span className="text-[9px] font-semibold px-1.5 py-0.5 uppercase tracking-wide text-[var(--color-accent)] bg-[var(--color-accent)]/15 flex-shrink-0">
              Primary
            </span>
            <SettingDropdown
              value={inbox.primary.level}
              options={options}
              onChange={(v) => void changePrimaryLevel(v as InboxLevel)}
              className={`flex-shrink-0 ${disabledCls ?? ''}`}
            />
          </div>
          <ManageToggles
            participant={inbox.primary}
            disabled={!canMutate || busy}
            onChange={(m, d) => void changeManage(primaryId, m, d)}
          />
        </div>

        {/* Grant rows */}
        {inbox.grants.map((g) => (
          <div key={g.projectId} className="px-3 py-2 space-y-2">
            <div className="flex items-center gap-2">
              <span className="text-xs text-[var(--color-text-primary)] truncate flex-1 min-w-0">
                {g.workspace ?? projectName(g.projectId)}
              </span>
              <SettingDropdown
                value={g.level}
                options={options}
                onChange={(v) => void changeGrantLevel(g.projectId, v as InboxLevel)}
                className={`flex-shrink-0 ${disabledCls ?? ''}`}
              />
              <button
                type="button"
                disabled={!canMutate || busy}
                onClick={() => void doMakePrimary(g.projectId)}
                title="Transfer Primary to this workspace"
                className="px-2 py-1 text-[10px] font-medium text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0 no-drag"
              >
                {pending === `primary:${g.projectId}` ? '…' : 'Make Primary'}
              </button>
              <button
                type="button"
                disabled={!canMutate || busy}
                onClick={() => void doRevoke(g.projectId)}
                title="Remove this workspace's access"
                className="px-2 py-1 text-[10px] font-medium text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0 no-drag"
              >
                {pending === g.projectId ? '…' : 'Remove'}
              </button>
            </div>
            <ManageToggles
              participant={g}
              disabled={!canMutate || busy}
              onChange={(m, d) => void changeManage(g.projectId, m, d)}
            />
          </div>
        ))}

        {inbox.grants.length === 0 && (
          <div className="px-3 py-2 text-[11px] text-[var(--color-text-muted)] italic">
            No other workspaces have access yet — only the Primary can use this inbox.
          </div>
        )}
      </div>

      {/* Add access */}
      <div className="flex items-center gap-2 flex-wrap pt-1">
        <SettingDropdown
          value={newProject}
          placeholder={addable.length === 0 ? 'No workspaces to add' : 'Add a workspace…'}
          options={addable.map((p) => ({ value: p.id, label: p.name }))}
          onChange={(v) => setNewProject(v)}
          menuAlign="left"
          className={
            !canMutate || busy || addable.length === 0 ? 'opacity-50 pointer-events-none' : undefined
          }
        />
        <SettingDropdown
          value={newLevel}
          options={options}
          onChange={(v) => setNewLevel(v as InboxLevel)}
          className={disabledCls}
        />
        <button
          type="button"
          disabled={!canMutate || busy || !newProject}
          onClick={() => void doAdd()}
          className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0 no-drag"
        >
          {pending === 'add' ? 'Adding…' : 'Add access'}
        </button>
      </div>

      {error && (
        <p className="text-[11px] text-[var(--color-status-error-soft)] break-words">{error}</p>
      )}
    </div>
  )
}
