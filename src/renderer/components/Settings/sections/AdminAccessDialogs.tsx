// Settings → Admin Access dialogs: Add a person, Reset password, and Name
// and picture. The frame, fields and the Reset password dialog itself are
// shared with User Access (AccessDialogs.tsx), so the two pages can't
// drift. A password K2 makes is shown once, in the open dialog only:
// closing drops it.

import React, { useEffect, useId, useState } from 'react'
import { Button, Callout, Checkbox, Input, Radio, Surface } from '@/components/ui'
import { SETTING_ROW_LABEL } from '../controls/SettingControls'
import { PERSON_NAME_MAX, realName } from '@/lib/person-name'
import { PresenceChip } from '@/components/Presence/PresenceChip'
import { PersonPictureControl } from '@/components/Presence/PersonPictureControl'
import { primaryScope } from '@/kessel/server-scope'
import {
  AccessSheet as AdminSheet,
  ConfirmSheet,
  CopyLine,
  DIALOG_FIELD as FIELD,
  DIALOG_LABEL as LABEL,
  DIALOG_MUTED as MUTED,
  FailedNote,
  Field,
  MUST_CHANGE_NOTE,
  PasswordChoice,
  PasswordResetDialog,
} from './AccessDialogs'
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
  policyWords,
  userPost,
  usernameProblem,
  type PasswordPolicy,
} from './admin-access-api'

export { ConfirmSheet }

/** What each role may do, in plain words (the add form and the role menu). */
export const ROLE_HELP: Record<K2Role, string> = {
  owner: 'Can do everything, including changing roles and removing people.',
  admin: 'Can add people and change this server’s settings. Can’t change roles or remove people.',
  member: 'Can use the workspaces and agents on this server. Can’t change who signs in.',
  voice_chat: 'Can join voice rooms and talk only with the agents and Gardens you share. Nothing else.',
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
          makeHint="K2 shows it once, after you add them, for you to copy and send."
          mode={mode}
          setMode={setMode}
          password={password}
          setPassword={setPassword}
          rules={policyWords(policy)}
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

        {error ? <FailedNote what={`Couldn’t add ${name}.`} detail={error} next="Nothing was added. Check the details above, then try again." /> : null}

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


/** Reset someone's K2 login password: the shared dialog, made by K2 (shown
 *  once) or typed, against this server's password rules. */
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
  return (
    <PasswordResetDialog
      who={who}
      intro={
        ownLogin
          ? 'This is your own login. Your current password stops working, and you keep the new one.'
          : `${who}’s current password stops working right away. The new one is temporary: when they next sign in, K2 asks them to choose their own.`
      }
      rules={policyWords(policy)}
      problem={(pw) => passwordProblem(pw, policy)}
      makeHint={`K2 shows it once, after the reset, for you to copy and send to ${who}.`}
      signIn={`They sign in to this server from their K2 app with it.`}
      onClose={onClose}
      onDone={onDone}
      submit={async (typed) => {
        const pw = typed ?? makePassword(policy)
        // No flag: the daemon decides (temporary, except the owner's own
        // linked login) and says which in its answer.
        const res = await userPost('/set-password', { username: who, password: pw })
        if (!res.ok) throw new Error(await errText(res))
        return { password: pw, temporary: await answeredTemporary(res, false) }
      }}
    />
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
        {error ? <FailedNote what={`Couldn’t save ${who}’s name.`} detail={error} next="The name is unchanged. Try again in a moment." /> : null}
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

