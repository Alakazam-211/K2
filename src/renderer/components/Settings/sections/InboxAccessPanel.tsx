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
//
// EFFECTIVE SEND (Appa/Baden, macOS 0.45.4): a level of Send only works when
// that workspace's agent-send gate allows it (`k2 mail sending`). The level
// and the gate stay two controls — the gate is per WORKSPACE and covers
// every inbox it uses (hosted and linked) plus hosted approval, so folding
// it into one inbox's level would silently change the others. Instead each
// Send row shows the daemon's effective `sendState` right under the level,
// and when it is blocked offers "Turn on sending" for that workspace.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { Button, OverflowMenu, Toggle, Tag } from '@/components/ui'
import {
  SaveError,
  SaveNote,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SettingDropdown,
  SettingsGroup,
  type SaveState,
} from '../controls/SettingControls'
import { InboxWakePanel } from './InboxWakePanel'
import {
  grantInboxAccess,
  mailErrorMessage,
  revokeInboxAccess,
  setInboxLevel,
  setInboxManage,
  setInboxPrimary,
  setWorkspaceAgentSend,
  type Inbox,
  type InboxLevel,
  type InboxParticipant,
  type SendState,
} from './email-api'

/** What a participant's Send level really does, in words. `null` = nothing
 *  to say (no Send level, or an older daemon without `sendState`). */
export function sendStateNote(
  state: SendState | undefined,
): { blocked: boolean; text: string } | null {
  switch (state) {
    case 'blocked_off':
      return {
        blocked: true,
        text: "Can't send yet: sending is off for this workspace's agents.",
      }
    // Daemons before MCS2 (linked approval hand-off) still answer this.
    case 'blocked_approval':
      return {
        blocked: true,
        text: "Can't send yet: this workspace sends only after your approval, which this account doesn't support. Turn sending on instead.",
      }
    case 'approval':
      return { blocked: false, text: 'Each email waits for your approval before it goes out.' }
    case 'on':
      return { blocked: false, text: 'Sending is on for its agents.' }
    default:
      return null
  }
}

/** Keep `sendState` right across an OPTIMISTIC level change (until the next
 *  catalog read brings the daemon's value). Mirrors `send_state` in
 *  crates/k2-daemon/src/mail/access.rs; unknown gate → leave it unset. */
function withLevel(p: InboxParticipant, level: InboxLevel): InboxParticipant {
  if (!p.agentSend) return { ...p, level, sendState: undefined }
  let sendState: SendState = 'no_send_level'
  if (level === 'send') {
    if (p.agentSend === 'on') sendState = 'on'
    // MCS2: linked inboxes queue for approval too (the approval hand-off).
    else if (p.agentSend === 'approval') sendState = 'approval'
    else sendState = 'blocked_off'
  }
  return { ...p, level, sendState }
}

const LEVEL_LABEL: Record<InboxLevel, string> = {
  read: 'Read',
  draft: 'Read and draft',
  send: 'Read, draft and send',
}

/** What a level lets a workspace's agents do, in one line. */
export const LEVEL_HINT: Record<InboxLevel, string> = {
  read: 'Its agents can read mail.',
  draft: 'Its agents can read mail and save draft replies for you to send.',
  send: 'Its agents can read mail, save drafts and send as this account.',
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

/** The control column every row shares: the level dropdown, then the row's
 *  "…" menu (or a slot its width, so switches below line up). */
const LEVEL_FIELD = 'w-52'
const MENU_SLOT = 'w-[30px] flex-shrink-0'

/** One workspace on the inbox: its name, what its level allows, whether a
 *  Send level really sends, and its folder and Trash switches. */
function ParticipantRow({
  participant,
  name,
  isPrimary,
  options,
  disabled,
  saved,
  sendPending,
  menu,
  onLevel,
  onManage,
  onTurnOnSending,
}: {
  participant: InboxParticipant
  name: string
  isPrimary: boolean
  options: { value: InboxLevel; label: string }[]
  disabled: boolean
  saved: boolean
  sendPending: boolean
  menu: React.ReactNode
  onLevel: (level: InboxLevel) => void
  onManage: (canManage: boolean, canDelete: boolean) => void
  onTurnOnSending: () => void
}): React.JSX.Element {
  const note = participant.level === 'send' ? sendStateNote(participant.sendState) : null
  const { canManage, canDelete } = participant
  return (
    <div className="px-4 py-3" data-testid={`access-row-${participant.projectId}`}>
      <div className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 min-w-0">
            <span className={`${SETTING_ROW_LABEL} truncate`}>{name}</span>
            {isPrimary && <Tag tone="accent">Primary</Tag>}
            <SaveNote state={saved ? 'saved' : 'idle'} />
          </div>
          <p className={SETTING_ROW_HINT}>{LEVEL_HINT[participant.level]}</p>
          {note && (
            <div
              className="flex items-center gap-3 mt-1"
              data-testid={`send-state-${participant.projectId}`}
              data-send-state={participant.sendState}
            >
              <span
                className={`type-hint leading-relaxed ${
                  note.blocked ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-text-secondary)]'
                }`}
              >
                {note.text}
              </span>
              {note.blocked && (
                <Button
                  className="flex-shrink-0"
                  disabled={disabled}
                  onClick={onTurnOnSending}
                  title={`Let ${name}'s agents send email`}
                >
                  {sendPending ? 'Turning on…' : 'Turn on sending'}
                </Button>
              )}
            </div>
          )}
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          <div className={`${LEVEL_FIELD} flex-shrink-0`}>
            <SettingDropdown
              value={participant.level}
              options={options}
              onChange={(v) => onLevel(v as InboxLevel)}
              disabled={disabled}
              ariaLabel={`What ${name} can do`}
              fullWidth
            />
          </div>
          <div className={MENU_SLOT}>{menu}</div>
        </div>
      </div>
      <div className="mt-2 space-y-2">
        <SwitchLine
          label="Sort mail into folders"
          checked={canManage}
          disabled={disabled}
          // Folders off → Trash off, in one server-valid change.
          onChange={(next) => onManage(next, next ? canDelete : false)}
          ariaLabel={`Let ${name} sort mail into folders`}
        />
        {canManage && (
          <SwitchLine
            label="Move mail to Trash"
            hint="Mail in Trash can still be recovered."
            checked={canDelete}
            disabled={disabled}
            onChange={(next) => onManage(true, next)}
            ariaLabel={`Let ${name} move mail to Trash`}
          />
        )}
      </div>
    </div>
  )
}

/** A switch under a workspace row, its switch on the dropdown's right edge. */
function SwitchLine({
  label,
  hint,
  checked,
  disabled,
  onChange,
  ariaLabel,
}: {
  label: string
  hint?: string
  checked: boolean
  disabled: boolean
  onChange: (next: boolean) => void
  ariaLabel: string
}): React.JSX.Element {
  return (
    <div className="flex items-center justify-between gap-6">
      <span className="type-hint text-[var(--color-text-secondary)] min-w-0">
        {label}
        {hint && <span className="text-[var(--color-text-secondary)]"> · {hint}</span>}
      </span>
      <div className="flex items-center gap-2 flex-shrink-0">
        <Toggle size="sm" checked={checked} disabled={disabled} onChange={onChange} aria-label={ariaLabel} />
        <div className={MENU_SLOT} />
      </div>
    </div>
  )
}

export function InboxAccessPanel({
  inbox,
  canMutate,
  patchInbox,
  card = false,
}: {
  inbox: Inbox
  canMutate: boolean
  /** Optimistically patch this inbox's row in the parent list (primary +
   *  grants) — no refetch (no fetch-in-render). */
  patchInbox: (address: string, patch: (i: Inbox) => Inbox) => void
  /** A Settings card group with its own title (Email Link). Without it the
   *  rows sit in a plain bordered list under a section label (Email Hosting's
   *  address rows). */
  card?: boolean
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const [result, setResult] = useState<SaveState>('idle')
  const [savedId, setSavedId] = useState<string | null>(null)
  const savedTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => () => {
    if (savedTimer.current) clearTimeout(savedTimer.current)
  }, [])
  // projectId in flight, or 'add', or `primary:<id>` for a transfer.
  const [pending, setPending] = useState<string | null>(null)
  const [newProject, setNewProject] = useState('')
  const [newLevel, setNewLevel] = useState<InboxLevel>('read')

  const options = useMemo(() => levelOptions(inbox.maxLevel), [inbox.maxLevel])
  const primaryId = inbox.primary.projectId
  const grantedIds = useMemo(() => new Set(inbox.grants.map((g) => g.projectId)), [inbox.grants])
  const projectName = useCallback(
    (id: string): string => projects.find((p) => p.id === id)?.name ?? id,
    [projects],
  )
  const nameOf = (p: InboxParticipant): string => p.workspace ?? projectName(p.projectId)

  /** One write: optimistic patch, the call, then "Saved" on that row or the
   *  reason it failed (the patch rolls back). */
  const run = useCallback(
    async (key: string, rowId: string, patch: (i: Inbox) => Inbox, call: () => Promise<unknown>): Promise<boolean> => {
      setResult('idle')
      setPending(key)
      const before = { primary: inbox.primary, grants: inbox.grants }
      patchInbox(inbox.address, patch)
      try {
        await call()
        setSavedId(rowId)
        if (savedTimer.current) clearTimeout(savedTimer.current)
        savedTimer.current = setTimeout(() => setSavedId(null), 2000)
        return true
      } catch (e) {
        patchInbox(inbox.address, (i) => ({ ...i, primary: before.primary, grants: before.grants }))
        setResult({ error: mailErrorMessage(e) })
        return false
      } finally {
        setPending(null)
      }
    },
    [inbox.address, inbox.primary, inbox.grants, patchInbox],
  )

  const changeLevel = (projectId: string, level: InboxLevel): void => {
    void run(
      projectId,
      projectId,
      (i) => ({
        ...i,
        primary: i.primary.projectId === projectId ? withLevel(i.primary, level) : i.primary,
        grants: i.grants.map((g) => (g.projectId === projectId ? withLevel(g, level) : g)),
      }),
      () => setInboxLevel({ address: inbox.address, project: projectId, level }),
    )
  }

  // Folder and Trash rights. Never sends Trash without folders (the daemon
  // refuses that pair).
  const changeManage = (projectId: string, canManage: boolean, canDelete: boolean): void => {
    const nextDelete = canManage ? canDelete : false
    const apply = (p: InboxParticipant): InboxParticipant =>
      p.projectId === projectId ? { ...p, canManage, canDelete: nextDelete } : p
    void run(
      `manage:${projectId}`,
      projectId,
      (i) => ({ ...i, primary: apply(i.primary), grants: i.grants.map(apply) }),
      () => setInboxManage({ address: inbox.address, project: projectId, canManage, canDelete: nextDelete }),
    )
  }

  // Turn a workspace's agent sending on (the fix for a blocked Send). Every
  // row of that workspace in this inbox flips to `on`.
  const turnOnSending = (projectId: string): void => {
    const apply = (p: InboxParticipant): InboxParticipant =>
      p.projectId === projectId
        ? { ...p, agentSend: 'on', sendState: p.level === 'send' ? 'on' : p.sendState }
        : p
    void run(
      `sending:${projectId}`,
      projectId,
      (i) => ({ ...i, primary: apply(i.primary), grants: i.grants.map(apply) }),
      () => setWorkspaceAgentSend({ workspace: projectId, agentSend: 'on' }),
    )
  }

  const removeAccess = async (g: InboxParticipant): Promise<void> => {
    const name = nameOf(g)
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Remove ${name}'s access?`,
      message: `${name}'s agents will no longer be able to use ${inbox.address}. You can give it access again at any time.`,
      confirmLabel: 'Remove access',
      destructive: true,
    })
    if (!ok) return
    void run(
      g.projectId,
      primaryId,
      (i) => ({ ...i, grants: i.grants.filter((x) => x.projectId !== g.projectId) }),
      () => revokeInboxAccess({ address: inbox.address, project: g.projectId }),
    )
  }

  // Hand the Primary to a workspace with access: it becomes the Primary
  // (keeping its level); the old Primary becomes an ordinary grant.
  const makePrimary = (projectId: string): void => {
    void run(
      `primary:${projectId}`,
      projectId,
      (i) => {
        const target = i.grants.find((g) => g.projectId === projectId)
        if (!target) return i
        return {
          ...i,
          primary: { ...target },
          grants: i.grants.filter((g) => g.projectId !== projectId).concat([{ ...i.primary }]),
        }
      },
      () => setInboxPrimary({ address: inbox.address, project: projectId }),
    )
  }

  const addAccess = async (): Promise<void> => {
    if (!newProject) return
    const projectId = newProject
    const level = newLevel
    const ok = await run(
      'add',
      projectId,
      (i) => {
        const exists = i.grants.some((g) => g.projectId === projectId)
        const grants = exists
          ? i.grants.map((g) => (g.projectId === projectId ? { ...g, level } : g))
          : [...i.grants, { projectId, workspace: projectName(projectId), level, canManage: false, canDelete: false }]
        return { ...i, grants }
      },
      () => grantInboxAccess({ address: inbox.address, project: projectId, level }),
    )
    if (ok) {
      setNewProject('')
      setNewLevel('read')
    }
  }

  // Candidates for a new grant: every workspace that isn't the Primary and
  // doesn't already hold a grant.
  const addable = useMemo(
    () => projects.filter((p) => p.id !== primaryId && !grantedIds.has(p.id)),
    [projects, primaryId, grantedIds],
  )
  const busy = pending !== null
  const locked = !canMutate || busy

  const rows = (
    <>
      <ParticipantRow
        participant={inbox.primary}
        name={nameOf(inbox.primary)}
        isPrimary
        options={options}
        disabled={locked}
        saved={savedId === primaryId}
        sendPending={pending === `sending:${primaryId}`}
        menu={null}
        onLevel={(l) => changeLevel(primaryId, l)}
        onManage={(m, d) => changeManage(primaryId, m, d)}
        onTurnOnSending={() => turnOnSending(primaryId)}
      />
      {inbox.grants.map((g) => (
        <ParticipantRow
          key={g.projectId}
          participant={g}
          name={nameOf(g)}
          isPrimary={false}
          options={options}
          disabled={locked}
          saved={savedId === g.projectId}
          sendPending={pending === `sending:${g.projectId}`}
          menu={
            <OverflowMenu
              label={`More actions for ${nameOf(g)}`}
              items={[
                {
                  id: 'primary',
                  label: 'Make Primary',
                  hint: 'It manages this account and who can use it.',
                  disabled: locked,
                  onSelect: () => makePrimary(g.projectId),
                },
                {
                  id: 'remove',
                  label: 'Remove access…',
                  danger: true,
                  group: true,
                  disabled: locked,
                  onSelect: () => void removeAccess(g),
                },
              ]}
            />
          }
          onLevel={(l) => changeLevel(g.projectId, l)}
          onManage={(m, d) => changeManage(g.projectId, m, d)}
          onTurnOnSending={() => turnOnSending(g.projectId)}
        />
      ))}
      <div className="px-4 py-3" data-testid="access-add">
        <div className="flex items-start justify-between gap-6">
          <div className="flex-1 min-w-0">
            <div className={SETTING_ROW_LABEL}>Give another workspace access</div>
            <div className={SETTING_ROW_HINT}>
              {addable.length === 0
                ? 'Every workspace already has access.'
                : 'Choose a workspace and what its agents may do.'}
            </div>
          </div>
          <div className="flex items-center gap-2 flex-shrink-0">
            <div className="w-44 flex-shrink-0">
              <SettingDropdown
                value={newProject}
                placeholder={addable.length === 0 ? 'No workspaces left' : 'Choose a workspace'}
                options={addable.map((p) => ({ value: p.id, label: p.name }))}
                onChange={(v) => setNewProject(v)}
                disabled={locked || addable.length === 0}
                ariaLabel="Workspace to give access"
                fullWidth
              />
            </div>
            <div className="w-44 flex-shrink-0">
              <SettingDropdown
                value={newLevel}
                options={options}
                onChange={(v) => setNewLevel(v as InboxLevel)}
                disabled={locked || addable.length === 0}
                ariaLabel="What it can do"
                fullWidth
              />
            </div>
            <Button
              className="flex-shrink-0"
              disabled={locked || !newProject}
              onClick={() => void addAccess()}
            >
              {pending === 'add' ? 'Giving access…' : 'Give access'}
            </Button>
          </div>
        </div>
      </div>
    </>
  )

  const failure = <SaveError state={result} />

  if (card) {
    return (
      <div className="space-y-1" data-settings-id="email-link.access">
        <SettingsGroup
          title="Workspace access"
          hint="Which workspaces' agents can use this account, and what they may do. The Primary workspace manages this list."
          card
        >
          {rows}
        </SettingsGroup>
        {failure}
        <InboxWakePanel inbox={inbox} canMutate={canMutate} />
      </div>
    )
  }
  return (
    <div className="space-y-2">
      <h3 className="type-section text-[var(--color-text-muted)] uppercase tracking-wider">Workspace access</h3>
      <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">{rows}</div>
      {failure}

      <InboxWakePanel inbox={inbox} canMutate={canMutate} />
    </div>
  )
}
