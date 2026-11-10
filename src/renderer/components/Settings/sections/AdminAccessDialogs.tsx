// Settings → Admin Access dialogs: Add a person, Reset password, and Name
// and picture. Each is the shared Dialog (scrim + frame) with labelled
// full-width fields, the server's reason when a write fails, and a done
// step that says what happened and what the person does next. A password
// K2 makes is shown once, in the open dialog only: closing drops it.

import React, { useEffect, useId, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Button, Callout, Checkbox, DialogFrame, DialogScrim, Input, Radio, Surface } from '@/components/ui'
import { useDialogEscape } from '../settings-esc'
import { SETTING_ROW_LABEL } from '../controls/SettingControls'
import { MUST_CHANGE_NOTE } from './AppUserDialogs'
import { PERSON_NAME_MAX, realName } from '@/lib/person-name'
import { PresenceChip } from '@/components/Presence/PresenceChip'
import { PersonPictureControl } from '@/components/Presence/PersonPictureControl'
import { primaryScope } from '@/kessel/server-scope'
import {
  addUserBody,
  splitShareList,
  temporaryDefaultFor,
  type K2Role,
  type K2User,
} from './connect-user-roles'
import {
  answeredTemporary,
  errText,
  makePassword,
  passwordProblem,
  userPost,
  usernameProblem,
  type PasswordPolicy,
} from './admin-access-api'

/** What each role may do, in plain words (the add form and the role menu). */
export const ROLE_HELP: Record<K2Role, string> = {
  owner: 'Can do everything, including changing roles and removing people.',
  admin: 'Can add people and change this server’s settings. Can’t change roles or remove people.',
  member: 'Can use the workspaces and agents on this server. Can’t change who signs in.',
  voice_chat: 'Can join voice rooms and talk only with the agents and Gardens you share. Nothing else.',
}

const LABEL = 'block type-row text-[var(--color-text-primary)] mb-1'
const FIELD = 'w-full type-control'
const MUTED = 'type-hint text-[var(--color-text-secondary)] leading-relaxed'

/** The shared Dialog with a title, a body that scrolls, and Esc to close
 *  (off while a write is in flight). */
function AdminSheet({
  title,
  intro,
  onClose,
  busy = false,
  testId,
  children,
}: {
  title: string
  intro?: React.ReactNode
  onClose: () => void
  busy?: boolean
  testId: string
  children: React.ReactNode
}): React.JSX.Element {
  const titleId = useId()
  useDialogEscape(onClose, !busy)
  // Portalled to <body>, so the scrim covers the whole window, not just the
  // Settings panel it opens from.
  return createPortal(
    <>
      <DialogScrim />
      <DialogFrame
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-testid={testId}
        className="w-[min(36rem,calc(100vw-2.5rem))] max-h-[calc(100vh-3rem)] flex flex-col"
      >
        <div className="min-h-0 overflow-y-auto p-5 space-y-4">
          <div>
            <h3 id={titleId} className="type-heading text-[var(--color-text-primary)]">
              {title}
            </h3>
            {intro ? <p className={`${MUTED} mt-1`}>{intro}</p> : null}
          </div>
          {children}
        </div>
      </DialogFrame>
    </>,
    document.body,
  )
}

/** A labelled field with its hint or problem under it. */
function Field({
  label,
  hint,
  problem,
  children,
  htmlFor,
}: {
  label: string
  hint?: React.ReactNode
  problem?: string | null
  children: React.ReactNode
  htmlFor: string
}): React.JSX.Element {
  return (
    <div>
      <label htmlFor={htmlFor} className={LABEL}>
        {label}
      </label>
      {children}
      {problem ? (
        <p className="type-hint text-[var(--color-status-error-soft)] mt-1" data-field-problem="">
          {problem}
        </p>
      ) : hint ? (
        <p className={`${MUTED} mt-1`}>{hint}</p>
      ) : null}
    </div>
  )
}

/** A failed write: what failed, the server's reason, the next step. */
function FailedNote({ what, detail, next }: { what: string; detail: string; next: string }): React.JSX.Element {
  // At the foot of a long form: bring it into view so the failure is seen.
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    ref.current?.scrollIntoView?.({ block: 'nearest' })
  }, [detail])
  return (
    <div ref={ref}>
      <Callout tone="error" role="alert" className="type-hint space-y-0.5" data-testid="dialog-error">
        <p className="font-medium">{what}</p>
        <p>The server said: {detail.trim().replace(/[.\s]+$/, '')}.</p>
        <p>{next}</p>
      </Callout>
    </div>
  )
}

/** A value to hand over (address, username, password) with Copy. */
function CopyLine({ label, value, mono = true, testId }: { label: string; value: string; mono?: boolean; testId?: string }): React.JSX.Element {
  const [copy, setCopy] = useState<'idle' | 'done' | 'failed'>('idle')
  const doCopy = (): void => {
    const clip = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
    if (!clip) return setCopy('failed')
    clip.writeText(value).then(
      () => setCopy('done'),
      () => setCopy('failed'),
    )
  }
  return (
    <div className="flex items-center justify-between gap-4 px-4 py-2.5">
      <div className="min-w-0">
        <p className="type-hint text-[var(--color-text-secondary)]">{label}</p>
        <p data-testid={testId} className={`type-row ${mono ? 'font-mono' : ''} break-all select-all text-[var(--color-text-primary)]`}>
          {value}
        </p>
        {copy === 'failed' ? <p className={MUTED}>Couldn’t copy. Select the text and copy it yourself.</p> : null}
      </div>
      <Button className="flex-shrink-0" aria-label={`Copy ${label.toLowerCase()}`} onClick={doCopy}>
        {copy === 'done' ? 'Copied' : 'Copy'}
      </Button>
    </div>
  )
}

/** Make one / Type one, and the typed field with Show. */
function PasswordChoice({
  name,
  mode,
  setMode,
  password,
  setPassword,
  policy,
  problem,
}: {
  name: string
  mode: 'make' | 'type'
  setMode: (m: 'make' | 'type') => void
  password: string
  setPassword: (v: string) => void
  policy: PasswordPolicy
  problem: string | null
}): React.JSX.Element {
  const [shown, setShown] = useState(false)
  const id = useId()
  return (
    <fieldset className="space-y-2">
      <legend className={LABEL}>Password</legend>
      <div role="radiogroup" aria-label="How to set the password" className="space-y-2">
        <label className="flex items-start gap-2 cursor-pointer">
          <Radio name={name} className="mt-0.5" checked={mode === 'make'} onChange={() => setMode('make')} aria-label="Make one for me" />
          <span>
            <span className={SETTING_ROW_LABEL}>Make one for me</span>
            <span className={`block ${MUTED}`}>K2 shows it once, after you add them, for you to copy and send.</span>
          </span>
        </label>
        <label className="flex items-start gap-2 cursor-pointer">
          <Radio name={name} className="mt-0.5" checked={mode === 'type'} onChange={() => setMode('type')} aria-label="Type one" />
          <span>
            <span className={SETTING_ROW_LABEL}>Type one</span>
            <span className={`block ${MUTED}`}>
              At least {policy.minLength} characters
              {policy.requireNumber ? ', with a number' : ''}
              {policy.requireUppercase ? ', an uppercase letter' : ''}
              {policy.requireSpecial ? ', a special character' : ''}.
            </span>
          </span>
        </label>
      </div>
      {mode === 'type' ? (
        <div className="pl-6">
          <label htmlFor={id} className="sr-only">
            Password
          </label>
          <div className="flex items-center gap-2">
            <Input
              id={id}
              type={shown ? 'text' : 'password'}
              autoComplete="new-password"
              className={FIELD}
              aria-label="Password"
              aria-invalid={problem ? true : undefined}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
            <Button variant="quiet" className="flex-shrink-0" onClick={() => setShown((s) => !s)}>
              {shown ? 'Hide' : 'Show'}
            </Button>
          </div>
          {problem ? <p className="type-hint text-[var(--color-status-error-soft)] mt-1">{problem}</p> : null}
        </div>
      ) : null}
    </fieldset>
  )
}

/** What the done step shows after a person is added. */
interface Added {
  username: string
  password: string | null
  temporary: boolean
  roleNote: string | null
}

/** Add a person: username, name, role, password. Done: the details to send,
 *  once, and what they do next. */
export function AddPersonDialog({
  roles,
  policy,
  canSetHigherRoles,
  serverAddress,
  taken,
  onClose,
  onAdded,
}: {
  /** The roles this viewer may give, lowest first. */
  roles: ReadonlyArray<{ value: K2Role; label: string }>
  policy: PasswordPolicy
  /** Admin or Owner is set after the add (owner only). */
  canSetHigherRoles: boolean
  /** Where they reach this server, or null when it has no address yet. */
  serverAddress: string | null
  /** Usernames already on this server. */
  taken: string[]
  onClose: () => void
  /** The list changed (refresh it). */
  onAdded: (username: string) => void
}): React.JSX.Element {
  const [username, setUsername] = useState('')
  const [firstName, setFirstName] = useState('')
  const [lastName, setLastName] = useState('')
  const [role, setRole] = useState<K2Role>(roles.some((r) => r.value === 'member') ? 'member' : roles[0]?.value ?? 'member')
  const [mode, setMode] = useState<'make' | 'type'>('make')
  const [password, setPassword] = useState('')
  const [temporary, setTemporary] = useState(temporaryDefaultFor(role))
  const [shareAgents, setShareAgents] = useState('')
  const [shareGardens, setShareGardens] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [added, setAdded] = useState<Added | null>(null)
  const ids = { user: useId(), first: useId(), last: useId(), agents: useId(), gardens: useId() }

  const name = username.trim().toLowerCase()
  const nameProblem = usernameProblem(username) ?? (name && taken.includes(name) ? `${name} is already on this server.` : null)
  const pwProblem = mode === 'type' && password ? passwordProblem(password, policy) : null
  const ready = !!name && !nameProblem && (mode === 'make' || (!!password && !pwProblem))

  const submit = async (): Promise<void> => {
    if (!ready || busy) return
    setBusy(true)
    setError(null)
    const pw = mode === 'make' ? makePassword(policy) : password
    try {
      const res = await userPost(
        '/add',
        addUserBody({
          username: name,
          password: pw,
          mustChangePassword: temporary,
          role,
          firstName: firstName.trim(),
          lastName: lastName.trim(),
          shareAgents: splitShareList(shareAgents),
          shareGardens: splitShareList(shareGardens),
        }),
      )
      if (!res.ok) {
        setError(await errText(res))
        return
      }
      const temp = await answeredTemporary(res, temporary)
      let roleNote: string | null = null
      // users/add makes a Member or a Voice & Chat only login; Admin and
      // Owner are set right after, through the same route as the list.
      if (canSetHigherRoles && (role === 'admin' || role === 'owner')) {
        const roleRes = await userPost('/set-role', { username: name, role })
        if (!roleRes.ok) {
          roleNote = `${name} was added as a Member: K2 couldn’t make them ${role === 'owner' ? 'an Owner' : 'an Admin'} (${(await errText(roleRes)).replace(/[.\s]+$/, '')}). Change their role in the list.`
        }
      }
      setPassword('')
      setAdded({ username: name, password: mode === 'make' ? pw : null, temporary: temp, roleNote })
      onAdded(name)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  if (added) {
    const who = added.username
    return (
      <AdminSheet title={`${who} can sign in now`} onClose={onClose} testId="person-added-dialog">
        <p className={MUTED} data-testid="added-user-msg">
          Send {who} these details yourself.
          {added.password ? ' K2 won’t show this password again: once you close this, it’s gone.' : ' Use the password you typed.'}
        </p>
        <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]">
          {serverAddress ? <CopyLine label="Server address" value={serverAddress} testId="added-address" /> : null}
          <CopyLine label="Username" value={who} />
          {added.password ? <CopyLine label="Password" value={added.password} testId="one-time-secret" /> : null}
        </Surface>
        {added.roleNote ? (
          <Callout tone="warn" role="alert" className="type-hint">
            {added.roleNote}
          </Callout>
        ) : null}
        <div>
          <p className={LABEL}>What {who} does next</p>
          <ol className={`${MUTED} list-decimal pl-5 space-y-0.5`}>
            <li>In K2 on their computer or phone, they add a server{serverAddress ? ' with the address above' : ''}.</li>
            {serverAddress ? null : (
              <li>This server has no internet address yet: turn on Settings → Tunnel, or give them its address on your network.</li>
            )}
            <li>They sign in with this username and password.</li>
            {added.temporary ? <li>K2 then asks them to choose their own password.</li> : null}
          </ol>
        </div>
        <div className="flex justify-end">
          <Button variant="accent" size="md" onClick={onClose} autoFocus>
            Done
          </Button>
        </div>
      </AdminSheet>
    )
  }

  return (
    <AdminSheet
      title="Add a person"
      intro="They sign in to this server from their own K2 app, with the username and password you set here."
      onClose={onClose}
      busy={busy}
      testId="add-person-dialog"
    >
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <Field
          label="Username"
          htmlFor={ids.user}
          hint="What they type to sign in. Letters, numbers, - and _. It can’t be changed later."
          problem={nameProblem}
        >
          <Input
            id={ids.user}
            autoFocus
            className={FIELD}
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            aria-invalid={nameProblem ? true : undefined}
            value={username}
            onChange={(e) => setUsername(e.target.value)}
          />
        </Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label="First name" htmlFor={ids.first}>
            <Input id={ids.first} className={FIELD} maxLength={PERSON_NAME_MAX} value={firstName} onChange={(e) => setFirstName(e.target.value)} />
          </Field>
          <Field label="Last name" htmlFor={ids.last}>
            <Input id={ids.last} className={FIELD} maxLength={PERSON_NAME_MAX} value={lastName} onChange={(e) => setLastName(e.target.value)} />
          </Field>
        </div>
        <p className={`${MUTED} -mt-2`}>Names are optional. People see them beside the username.</p>

        <fieldset className="space-y-2">
          <legend className={LABEL}>Role</legend>
          <div role="radiogroup" aria-label="Role" className="space-y-2">
            {roles.map((r) => (
              <label key={r.value} className="flex items-start gap-2 cursor-pointer" data-testid={`role-choice-${r.value}`}>
                <Radio
                  name="add-person-role"
                  className="mt-0.5"
                  checked={role === r.value}
                  aria-label={`${r.label}: ${ROLE_HELP[r.value]}`}
                  onChange={() => {
                    setRole(r.value)
                    setTemporary(temporaryDefaultFor(r.value))
                  }}
                />
                <span>
                  <span className={SETTING_ROW_LABEL}>{r.label}</span>
                  <span className={`block ${MUTED}`}>{ROLE_HELP[r.value]}</span>
                </span>
              </label>
            ))}
          </div>
        </fieldset>

        {role === 'voice_chat' ? (
          <div className="space-y-3 pl-6" data-testid="new-user-shares">
            <Field label="Agents they can talk to" htmlFor={ids.agents} hint="Agent addresses, separated by commas. You can change these later.">
              <Input id={ids.agents} className={FIELD} value={shareAgents} onChange={(e) => setShareAgents(e.target.value)} />
            </Field>
            <Field label="Gardens they can view" htmlFor={ids.gardens} hint="Garden ids, separated by commas.">
              <Input id={ids.gardens} className={FIELD} value={shareGardens} onChange={(e) => setShareGardens(e.target.value)} />
            </Field>
          </div>
        ) : null}

        <PasswordChoice
          name="add-person-password"
          mode={mode}
          setMode={setMode}
          password={password}
          setPassword={setPassword}
          policy={policy}
          problem={pwProblem}
        />
        <label className="flex items-start gap-2 cursor-pointer" data-testid="new-user-temporary">
          <Checkbox className="mt-0.5" checked={temporary} onChange={(e) => setTemporary(e.target.checked)} aria-label="Temporary password" />
          <span>
            <span className={SETTING_ROW_LABEL}>Ask them to choose their own password</span>
            <span className={`block ${MUTED}`}>
              {temporary ? MUST_CHANGE_NOTE : 'Off: they keep the password you set.'}
            </span>
          </span>
        </label>

        {error ? <FailedNote what={`Couldn’t add ${name}.`} detail={error} next="Nothing was added. Check the details and try again." /> : null}

        <div className="flex justify-end gap-2 pt-1">
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" variant="accent" size="md" disabled={!ready || busy}>
            {busy ? 'Adding…' : 'Add person'}
          </Button>
        </div>
      </form>
    </AdminSheet>
  )
}

/** Reset someone's password: made by K2 (shown once) or typed. */
export function ResetPasswordDialog({
  user,
  policy,
  ownLogin,
  onClose,
  onDone,
}: {
  user: K2User
  policy: PasswordPolicy
  /** The owner's own linked login: the password is kept, not temporary. */
  ownLogin: boolean
  onClose: () => void
  onDone: () => void
}): React.JSX.Element {
  const who = user.username
  const [mode, setMode] = useState<'make' | 'type'>('make')
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState<{ password: string | null; temporary: boolean } | null>(null)
  const pwProblem = mode === 'type' && password ? passwordProblem(password, policy) : null
  const ready = mode === 'make' || (!!password && !pwProblem)

  const submit = async (): Promise<void> => {
    if (!ready || busy) return
    setBusy(true)
    setError(null)
    const pw = mode === 'make' ? makePassword(policy) : password
    try {
      // No flag: the daemon decides (temporary, except the owner's own
      // linked login) and says which in its answer.
      const res = await userPost('/set-password', { username: who, password: pw })
      if (!res.ok) {
        setError(await errText(res))
        return
      }
      const temporary = await answeredTemporary(res, false)
      setPassword('')
      setDone({ password: mode === 'make' ? pw : null, temporary })
      onDone()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  if (done) {
    return (
      <AdminSheet title={`New password for ${who}`} onClose={onClose} testId="reset-done-dialog">
        <p className={MUTED} data-testid="reset-password-msg">
          {done.temporary ? `Password reset for ${who}. ${MUST_CHANGE_NOTE}` : `Password reset for ${who}.`}
        </p>
        {done.password ? (
          <>
            <Surface chamfer material="tile">
              <CopyLine label="Password" value={done.password} testId="one-time-secret" />
            </Surface>
            <p className={MUTED}>Send it to {who} yourself. K2 won’t show it again: once you close this, it’s gone.</p>
          </>
        ) : null}
        <div className="flex justify-end">
          <Button variant="accent" size="md" onClick={onClose} autoFocus>
            Done
          </Button>
        </div>
      </AdminSheet>
    )
  }

  return (
    <AdminSheet
      title={`Reset ${who}’s password`}
      intro={`Their old password stops working. ${ownLogin ? 'Your own login: you keep this password.' : MUST_CHANGE_NOTE}`}
      onClose={onClose}
      busy={busy}
      testId={`reset-form-${who}`}
    >
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <PasswordChoice
          name="reset-password"
          mode={mode}
          setMode={setMode}
          password={password}
          setPassword={setPassword}
          policy={policy}
          problem={pwProblem}
        />
        {error ? <FailedNote what={`Couldn’t reset ${who}’s password.`} detail={error} next="Their password is unchanged. Try again." /> : null}
        <div className="flex justify-end gap-2 pt-1">
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" variant="accent" size="md" disabled={!ready || busy}>
            {busy ? 'Resetting…' : 'Reset password'}
          </Button>
        </div>
      </form>
    </AdminSheet>
  )
}

/** Someone's first and last name (saved with Save) and picture (saved as
 *  soon as it's picked). */
export function NameAndPictureDialog({
  user,
  onClose,
  onSave,
}: {
  user: K2User
  onClose: () => void
  /** Saves the names; resolves to an error message, or null. */
  onSave: (firstName: string, lastName: string) => Promise<string | null>
}): React.JSX.Element {
  const who = user.username
  const [firstName, setFirstName] = useState(user.firstName ?? '')
  const [lastName, setLastName] = useState(user.lastName ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const ids = { first: useId(), last: useId() }
  useEffect(() => setError(null), [firstName, lastName])

  const submit = async (): Promise<void> => {
    setBusy(true)
    const err = await onSave(firstName.trim(), lastName.trim())
    setBusy(false)
    if (err) setError(err)
    else onClose()
  }

  return (
    <AdminSheet title={`Name and picture for ${who}`} onClose={onClose} busy={busy} testId={`name-form-${who}`}>
      <div className="flex items-center gap-3">
        <PresenceChip name={who} role="member" size={40} color={user.color ?? null} names={user} pictureScope={primaryScope()} title="" />
        <div className="min-w-0 space-y-1">
          <p className={SETTING_ROW_LABEL}>{realName(user) ?? who}</p>
          <PersonPictureControl scope={primaryScope()} login={who} person={who} look="buttons" testId={`user-picture-${who}`} />
        </div>
      </div>
      <p className={MUTED}>A picture saves as soon as you pick it. PNG, JPEG or WebP, up to 2 MB.</p>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <div className="grid grid-cols-2 gap-3">
          <Field label="First name" htmlFor={ids.first}>
            <Input id={ids.first} autoFocus className={FIELD} aria-label={`${who} first name`} maxLength={PERSON_NAME_MAX} value={firstName} onChange={(e) => setFirstName(e.target.value)} />
          </Field>
          <Field label="Last name" htmlFor={ids.last}>
            <Input id={ids.last} className={FIELD} aria-label={`${who} last name`} maxLength={PERSON_NAME_MAX} value={lastName} onChange={(e) => setLastName(e.target.value)} />
          </Field>
        </div>
        <p className={`${MUTED} -mt-2`}>Leave both empty to show only the username.</p>
        {error ? <FailedNote what={`Couldn’t save ${who}’s name.`} detail={error} next="The name is unchanged. Try again." /> : null}
        <div className="flex justify-end gap-2 pt-1">
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" variant="accent" size="md" disabled={busy}>
            {busy ? 'Saving…' : 'Save name'}
          </Button>
        </div>
      </form>
    </AdminSheet>
  )
}

