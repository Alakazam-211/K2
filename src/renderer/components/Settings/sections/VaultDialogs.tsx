// Settings → Vault dialogs: add, rotate, show (reveal), sites, how agents
// get it, share, delete, history, and recovery-key setup. Each one is the
// Settings dialog frame shared with Admin Access (AccessDialogs.tsx): one
// width, centred, labelled full-width fields, the server's reason when a
// write fails.
//
// The security rules in VaultSection.tsx's header hold here:
// - a value is on screen only in the Show dialog, after a confirm, masked
//   until "Show", and gone after 30 seconds or when the dialog closes;
// - a pasted value (add, rotate) is read from its field once, on submit,
//   sent as `valueB64`, and the field is emptied; it never enters React
//   state (Show / Hide only flips the field's masking);
// - a typed confirm password is read once and the field emptied;
// - the 24 recovery words live in the dialog's state only, are never
//   copied to the clipboard, and are gone once the check step starts.

import React, { useEffect, useId, useRef, useState } from 'react'
import { Button, Callout, Input, Radio, Spinner, Surface, Textarea } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import { useProjectsStore } from '@/stores/projects'
import { siteLabel } from '@/components/SessionView/SecretCard'
import { SettingDropdown, SETTING_ROW_HINT, SETTING_ROW_LABEL } from '../controls/SettingControls'
import { AccessSheet as Sheet, CopyLine, DIALOG_FIELD as FIELD, DIALOG_LABEL as LABEL, DIALOG_MUTED as MUTED, FailedNote, Field } from './AccessDialogs'
import { fromBase64, toBase64, vaultApi, type VaultAuditRow, type VaultPreset, type VaultRecoveryChallenge, type VaultSecret } from './vault-api'
import {
  ADD_KINDS,
  actorWords,
  agentUseWords,
  auditActionWords,
  ceilingChoices,
  deliveryChoices,
  deliveryLabel,
  deliveryOptions,
  deliveryReadable,
  isMultilineKind,
  REVEAL_SHOW_MS,
  sourceWords,
  whenWords,
  withCurrent,
} from './vault-copy'

/** A dropdown with its label above and a hint under it. */
function Pick({ label, hint, children }: { label: string; hint?: React.ReactNode; children: React.ReactNode }): React.JSX.Element {
  return (
    <div>
      <p className={LABEL}>{label}</p>
      {children}
      {hint ? <p className={`${MUTED} mt-1`}>{hint}</p> : null}
    </div>
  )
}

/** "Agents can read this value" under a delivery choice that lets them. */
function ReadableNote({ text }: { text: string }): React.JSX.Element {
  return (
    <Callout tone="warn" className="type-hint" data-testid="vault-readable-note">
      {text}
    </Callout>
  )
}

/** Copy a value; says when it couldn't. */
function CopyButton({ text, label = 'Copy' }: { text: string; label?: string }): React.JSX.Element {
  const [state, setState] = useState<'idle' | 'done' | 'failed'>('idle')
  useEffect(() => {
    if (state !== 'done') return
    const t = setTimeout(() => setState('idle'), 1500)
    return () => clearTimeout(t)
  }, [state])
  return (
    <Button
      onClick={() => {
        const clip = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
        if (!clip) return setState('failed')
        clip.writeText(text).then(
          () => setState('done'),
          () => setState('failed'),
        )
      }}
    >
      {state === 'done' ? 'Copied' : state === 'failed' ? 'Couldn’t copy' : label}
    </Button>
  )
}

// ── Value field ─────────────────────────────────────────────────────

type ValueRef = React.RefObject<HTMLInputElement | HTMLTextAreaElement | null>

/** A value field whose text never enters React state. Show / Hide flips
 *  the masking only. A key (several lines) gets a masked text box and a
 *  file picker, so its line breaks survive. */
export function ValueField({
  fieldRef,
  id,
  label,
  onPresence,
  multiline = false,
}: {
  fieldRef: ValueRef
  id: string
  label: string
  onPresence: (has: boolean) => void
  multiline?: boolean
}): React.JSX.Element {
  const [shown, setShown] = useState(false)
  const fileRef = useRef<HTMLInputElement>(null)
  const toggle = (
    <Button variant="quiet" className="flex-shrink-0" aria-pressed={shown} onClick={() => setShown((s) => !s)}>
      {shown ? 'Hide' : 'Show'}
    </Button>
  )
  if (multiline) {
    return (
      <div className="space-y-2">
        <Textarea
          id={id}
          ref={fieldRef as React.RefObject<HTMLTextAreaElement | null>}
          autoComplete="off"
          spellCheck={false}
          aria-label={label}
          placeholder="Paste the whole key, BEGIN and END lines included"
          rows={5}
          onChange={(e) => onPresence(e.currentTarget.value !== '')}
          className="font-mono"
          style={{ WebkitTextSecurity: shown ? 'none' : 'disc' } as React.CSSProperties}
          data-testid="vault-value-multiline"
        />
        <div className="flex items-center gap-2">
          {toggle}
          <Button variant="quiet" onClick={() => fileRef.current?.click()}>
            Load from a file…
          </Button>
          <input
            ref={fileRef}
            type="file"
            className="sr-only"
            tabIndex={-1}
            aria-label={`${label} from a file`}
            onChange={async (e) => {
              const f = e.currentTarget.files?.[0]
              const ta = fieldRef.current
              if (f && ta) {
                ta.value = await f.text()
                onPresence(ta.value !== '')
              }
              e.currentTarget.value = ''
            }}
          />
        </div>
      </div>
    )
  }
  return (
    <div className="flex items-center gap-2">
      <Input
        id={id}
        ref={fieldRef as React.RefObject<HTMLInputElement | null>}
        type={shown ? 'text' : 'password'}
        autoComplete="off"
        spellCheck={false}
        aria-label={label}
        placeholder="Paste the value"
        onChange={(e) => onPresence(e.currentTarget.value !== '')}
        className={`${FIELD} font-mono`}
      />
      {toggle}
    </div>
  )
}

// ── Confirm it's you ────────────────────────────────────────────────

type Ask = { step: 'idle' } | { step: 'asking' } | { step: 'mac' } | { step: 'password'; method: string } | { step: 'none' }

/** The vault's person confirm (the Mac's own sheet, or a password): `begin`
 *  resolves with a token, or null when it now waits for a password (or this
 *  server offers no way to confirm); `withPassword` sends the typed one. */
function useVaultConfirm(action: string, secretId: string | undefined, label: string) {
  const [ask, setAsk] = useState<Ask>({ step: 'idle' })
  const pwRef = useRef<HTMLInputElement>(null)
  const [hasPw, setHasPw] = useState(false)
  const finish = async (method: string, password?: string): Promise<string> => {
    const { token } = await vaultApi.confirmFinish({
      action,
      ...(secretId !== undefined ? { secretId } : {}),
      method,
      label,
      ...(password !== undefined ? { password } : {}),
    })
    return token
  }
  const begin = async (): Promise<string | null> => {
    setAsk({ step: 'asking' })
    try {
      const { methods } = await vaultApi.confirmStart(action, secretId)
      if (methods.includes('local-auth')) {
        setAsk({ step: 'mac' })
        const token = await finish('local-auth')
        setAsk({ step: 'idle' })
        return token
      }
      const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
      setAsk(pw ? { step: 'password', method: pw } : { step: 'none' })
      return null
    } catch (e) {
      setAsk({ step: 'idle' })
      throw e
    }
  }
  const withPassword = async (): Promise<string | null> => {
    const el = pwRef.current
    if (ask.step !== 'password' || !el) return null
    const pw = el.value
    el.value = ''
    setHasPw(false)
    const token = await finish(ask.method, pw)
    setAsk({ step: 'idle' })
    return token
  }
  const reset = (): void => setAsk({ step: 'idle' })
  return { ask, begin, withPassword, reset, pwRef, hasPw, setHasPw }
}

type Confirm = ReturnType<typeof useVaultConfirm>

/** What the confirm step shows: waiting, the Mac's sheet, the password
 *  field, or how to make confirming possible. */
function ConfirmStep({ c, testId, other }: { c: Confirm; testId: string; other?: { text: string; command: string } }): React.JSX.Element | null {
  const id = useId()
  if (c.ask.step === 'asking') return <Waiting text="Asking the vault how to confirm it’s you…" />
  if (c.ask.step === 'mac') return <Waiting text="Confirm on your Mac: use Touch ID or your Mac’s password in the window that opened." />
  if (c.ask.step === 'password') {
    return (
      <Field
        label={c.ask.method === 'connect-password' ? 'Your K2 Connect password' : 'Your vault password'}
        hint="K2 checks it once and doesn’t keep it."
        htmlFor={id}
      >
        <Input
          id={id}
          ref={c.pwRef}
          type="password"
          autoComplete="current-password"
          autoFocus
          data-testid={testId}
          className={FIELD}
          onChange={(e) => c.setHasPw(e.currentTarget.value !== '')}
        />
      </Field>
    )
  }
  if (c.ask.step === 'none') {
    return (
      <Callout tone="warn" role="alert" className="type-hint space-y-1" data-testid="vault-no-confirm">
        <p className="font-medium">K2 can’t confirm it’s you on this server yet.</p>
        <p>Set a vault password first. In a terminal outside K2, run:</p>
        <p className="font-mono select-all">k2 vault confirm-password set</p>
        {other ? (
          <>
            <p>{other.text}</p>
            <p className="font-mono select-all">{other.command}</p>
          </>
        ) : null}
      </Callout>
    )
  }
  return null
}

function Waiting({ text }: { text: string }): React.JSX.Element {
  return (
    <p role="status" className={`flex items-start gap-2 ${MUTED}`}>
      <Spinner className="mt-0.5" />
      {text}
    </p>
  )
}

// ── Show (reveal) ───────────────────────────────────────────────────

/** Show a value after a confirm. The value lives in this dialog only: masked
 *  until Show, gone after 30 seconds or when the dialog closes. */
export function RevealDialog({ secret, onClose }: { secret: VaultSecret; onClose: () => void }): React.JSX.Element {
  const formId = useId()
  const c = useVaultConfirm('reveal', secret.id, secret.name)
  const [shown, setShown] = useState<{ value: string; version: number } | null>(null)
  const [visible, setVisible] = useState(false)
  const [hidden, setHidden] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const reveal = async (token: string): Promise<void> => {
    const r = await vaultApi.reveal(secret.id, token)
    setShown({ value: fromBase64(r.valueB64), version: r.version })
    setVisible(false)
  }
  const start = async (): Promise<void> => {
    setError(null)
    setHidden(false)
    try {
      const token = await c.begin()
      if (token) await reveal(token)
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }
  const submit = async (): Promise<void> => {
    setError(null)
    setBusy(true)
    try {
      const token = await c.withPassword()
      if (token) await reveal(token)
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  // Ask once, when the dialog opens.
  const asked = useRef(false)
  useEffect(() => {
    if (asked.current) return
    asked.current = true
    void start()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  // Gone after 30 seconds.
  useEffect(() => {
    if (!shown) return
    const t = setTimeout(() => {
      setShown(null)
      setVisible(false)
      setHidden(true)
    }, REVEAL_SHOW_MS)
    return () => clearTimeout(t)
  }, [shown])

  const title = `Show ${secret.name}`
  if (shown) {
    return (
      <Sheet
        title={title}
        onClose={onClose}
        testId="vault-reveal-dialog"
        footer={
          <>
            <Button variant="accent" size="md" onClick={onClose}>
              Done
            </Button>
          </>
        }
      >
        <Surface chamfer material="tile" className="px-4 py-3 space-y-3" data-testid="vault-reveal-panel">
          <p className="font-mono type-row text-[var(--color-text-primary)] break-all select-all" data-testid="vault-reveal-value">
            {visible ? shown.value : '•'.repeat(Math.min(24, Math.max(8, shown.value.length)))}
          </p>
          <div className="flex items-center gap-2">
            <Button aria-pressed={visible} onClick={() => setVisible((v) => !v)}>
              {visible ? 'Hide' : 'Show'}
            </Button>
            <CopyButton text={shown.value} />
          </div>
        </Surface>
        <p className={MUTED}>Version {shown.version}. It hides again after 30 seconds, or when you close this.</p>
      </Sheet>
    )
  }
  if (hidden) {
    return (
      <Sheet
        title={title}
        onClose={onClose}
        testId="vault-reveal-dialog"
        footer={
          <>
            <Button variant="ghost" size="md" onClick={onClose}>
              Close
            </Button>
            <Button variant="accent" size="md" onClick={() => void start()}>
              Show again
            </Button>
          </>
        }
      >
        <p className={MUTED} role="status">
          The value is hidden again: K2 shows it for 30 seconds at a time.
        </p>
      </Sheet>
    )
  }
  const pw = c.ask.step === 'password'
  return (
    <Sheet
      title={title}
      intro="Confirm it’s you to see the value. It shows here only, for 30 seconds."
      onClose={onClose}
      busy={busy}
      testId="vault-reveal-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          {pw ? (
            <Button type="submit" form={formId} variant="accent" size="md" disabled={!c.hasPw || busy}>
              {busy ? 'Confirming…' : 'Confirm'}
            </Button>
          ) : error && c.ask.step === 'idle' ? (
            <Button variant="accent" size="md" onClick={() => void start()}>
              Try again
            </Button>
          ) : null}
        </>
      }
    >
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          if (pw && c.hasPw && !busy) void submit()
        }}
      >
        <ConfirmStep c={c} testId="vault-reveal-password" />
        {error ? (
          <FailedNote
            what="Couldn’t show the value."
            detail={error}
            next={pw ? 'Nothing was shown. Check the password, then confirm again.' : 'Nothing was shown. Try again in a moment.'}
          />
        ) : null}
      </form>
    </Sheet>
  )
}

// ── Add ─────────────────────────────────────────────────────────────

/** Add a secret: name, what it is, who may use it, the sites it may go to,
 *  how agents get it, and the value. */
export function AddSecretDialog({
  presets,
  member,
  onClose,
  onSaved,
}: {
  presets: VaultPreset[]
  /** A Member keeps secrets for a workspace; all agents needs an Admin. */
  member: boolean
  onClose: () => void
  onSaved: (note: string) => void
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const [name, setName] = useState('')
  const [scope, setScope] = useState<'workspace' | 'server'>('workspace')
  const [workspace, setWorkspace] = useState(projects[0]?.id ?? '')
  const [preset, setPreset] = useState('')
  const [sites, setSites] = useState('')
  const [delivery, setDelivery] = useState('fetch')
  const [kind, setKind] = useState('api_key')
  const fieldRef = useRef<HTMLInputElement | HTMLTextAreaElement>(null)
  const [has, setHas] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const ids = { name: useId(), sites: useId(), value: useId() }
  const formId = useId()

  const presetRow = presets.find((p) => p.id === preset) ?? null
  const presetSites = (presetRow?.hosts ?? []).map(siteLabel).join(', ')
  const siteList = sites
    .split(/[,\s]+/)
    .map((s) => s.trim())
    .filter(Boolean)
  const needsSite = delivery === 'fetch' && siteList.length === 0
  const canSave = has && name.trim() !== '' && !needsSite && !busy && (scope === 'server' || workspace !== '')
  const wsName = (id: string): string => {
    const p = projects.find((x) => x.id === id)
    return p?.handle || p?.name || 'the workspace'
  }

  const save = async (): Promise<void> => {
    const el = fieldRef.current
    if (!el || !canSave) return
    setBusy(true)
    setError(null)
    try {
      const valueB64 = toBase64(el.value)
      await vaultApi.set({
        name: name.trim(),
        scope,
        ...(scope === 'workspace' ? { workspace } : {}),
        valueB64,
        kind,
        delivery,
        ...(preset ? { preset } : {}),
        // The preset's own sites go in by the preset; send the field only
        // when the person changed it.
        ...(sites !== presetSites ? { hosts: siteList } : {}),
        mode: 'create',
      })
      el.value = ''
      setHas(false)
      onSaved(
        scope === 'server'
          ? `${name.trim()} is saved on the server. Share it with the workspaces that should use it.`
          : `${name.trim()} is saved. Agents in ${wsName(workspace)} can use it from their next call.`,
      )
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Sheet
      title="Add a secret"
      intro="Agents use it by name. K2 sends it only to the sites you list, and logs every use."
      onClose={onClose}
      busy={busy}
      testId="vault-add-form"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={formId} variant="accent" size="md" disabled={!canSave} data-testid="vault-add-save">
            {busy ? 'Saving…' : 'Save secret'}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void save()
        }}
      >
        <Field label="Name" hint="Letters, numbers and underscores, like GITHUB_TOKEN." htmlFor={ids.name}>
          <Input
            id={ids.name}
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="GITHUB_TOKEN"
            className={`${FIELD} font-mono`}
          />
        </Field>
        <Pick label="What it is">
          <SettingDropdown
            fullWidth
            menuAlign="left"
            ariaLabel="What it is"
            triggerData={{ 'data-testid': 'vault-add-kind' }}
            value={kind}
            options={ADD_KINDS}
            onChange={(k) => {
              // A re-pick of the same kind keeps the value and delivery.
              if (k === kind) return
              setKind(k)
              // The value field may change shape (one line vs a key): start empty.
              setHas(false)
              // agent is for SSH keys only; an SSH key starts as a file (the daemon's default).
              if (k === 'ssh_key') setDelivery('file')
              else if (delivery === 'agent') setDelivery('fetch')
            }}
          />
        </Pick>
        <fieldset className="space-y-2">
          <legend className={LABEL}>Who can use it</legend>
          <label className="flex items-start gap-2 cursor-pointer">
            <Radio name="vault-add-scope" className="mt-0.5" checked={scope === 'workspace'} onChange={() => setScope('workspace')} aria-label="One workspace’s agents" />
            <span>
              <span className={SETTING_ROW_LABEL}>One workspace’s agents</span>
            </span>
          </label>
          {scope === 'workspace' && (
            <div className="pl-6">
              <SettingDropdown
                fullWidth
                menuAlign="left"
                ariaLabel="Workspace"
                value={workspace}
                placeholder="Choose a workspace"
                options={projects.map((p) => ({ value: p.id, label: p.handle || p.name }))}
                onChange={setWorkspace}
              />
            </div>
          )}
          <label className={`flex items-start gap-2 ${member ? 'cursor-not-allowed' : 'cursor-pointer'}`}>
            <Radio
              name="vault-add-scope"
              className="mt-0.5"
              checked={scope === 'server'}
              disabled={member}
              onChange={() => setScope('server')}
              aria-label="The server (share it with agents after)"
            />
            <span>
              <span className={SETTING_ROW_LABEL}>The server (share it with agents after)</span>
              {member ? <span className={`block ${MUTED}`}>Only an Owner or Admin can keep a secret on the server.</span> : null}
            </span>
          </label>
        </fieldset>
        {presets.length > 0 && (
          <Pick label="Service" hint="Picking one fills in its sites and how K2 sends it.">
            <SettingDropdown
              fullWidth
              menuAlign="left"
              ariaLabel="Service"
              triggerData={{ 'data-testid': 'vault-add-preset' }}
              value={preset}
              options={[{ value: '', label: 'None: I’ll list the sites' }, ...presets.map((p) => ({ value: p.id, label: p.label }))]}
              onChange={(id) => {
                // A re-pick of the same service keeps sites typed since.
                if (id === preset) return
                setPreset(id)
                const p = presets.find((x) => x.id === id)
                setSites((p?.hosts ?? []).map(siteLabel).join(', '))
              }}
            />
          </Pick>
        )}
        <Field
          label="Only send it to"
          hint={needsSite ? 'Add at least one site, like api.example.com, or choose a way agents can read it below.' : 'Separate sites with commas. K2 sends it nowhere else.'}
          htmlFor={ids.sites}
        >
          <Input
            id={ids.sites}
            value={sites}
            onChange={(e) => setSites(e.target.value)}
            placeholder="api.example.com"
            className={`${FIELD} font-mono`}
            data-testid="vault-add-sites"
          />
        </Field>
        <Pick label="How agents get it">
          <SettingDropdown
            fullWidth
            menuAlign="left"
            ariaLabel="How agents get it"
            triggerData={{ 'data-testid': 'vault-add-delivery' }}
            value={delivery}
            options={deliveryOptions(deliveryChoices(kind))}
            onChange={setDelivery}
          />
        </Pick>
        {deliveryReadable(delivery) && <ReadableNote text="Agents can see this value. Choose “K2 adds it to calls” to keep it hidden from them." />}
        <Field label="Value" hint="A key can still do anything its service allows. Use a restricted key if you can." htmlFor={ids.value}>
          <ValueField fieldRef={fieldRef} id={ids.value} label="Value" onPresence={setHas} multiline={isMultilineKind(kind)} />
        </Field>
        {error ? <FailedNote what="Couldn’t save the secret." detail={error} next="Nothing was saved. Check the name and sites, then try again." /> : null}
      </form>
    </Sheet>
  )
}

// ── Rotate ──────────────────────────────────────────────────────────

export function RotateDialog({ secret, onClose, onSaved }: { secret: VaultSecret; onClose: () => void; onSaved: (note: string) => void }): React.JSX.Element {
  const formId = useId()
  const fieldRef = useRef<HTMLInputElement | HTMLTextAreaElement>(null)
  const id = useId()
  const [has, setHas] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const rotate = async (): Promise<void> => {
    const el = fieldRef.current
    if (!el || !el.value || busy) return
    setBusy(true)
    setError(null)
    try {
      const valueB64 = toBase64(el.value)
      el.value = ''
      setHas(false)
      const r = await vaultApi.rotate({ id: secret.id, valueB64 })
      onSaved(`${secret.name} now uses the new value${r?.version ? ` (version ${r.version})` : ''}. Agents use it from their next call.`)
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <Sheet
      title={`Rotate ${secret.name}`}
      intro={`Paste the new value from ${secret.provider || 'the service'}. Agents use it from their next call, and the history keeps a note of the change.`}
      onClose={onClose}
      busy={busy}
      testId="vault-rotate-panel"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={formId} variant="accent" size="md" disabled={busy || !has}>
            {busy ? 'Rotating…' : 'Rotate'}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void rotate()
        }}
      >
        <Field label="New value" htmlFor={id}>
          <ValueField fieldRef={fieldRef} id={id} label={`New value for ${secret.name}`} onPresence={setHas} multiline={isMultilineKind(secret.kind)} />
        </Field>
        {error ? <FailedNote what="Couldn’t rotate it." detail={error} next="The old value still works. Paste the new one again, then try again." /> : null}
      </form>
    </Sheet>
  )
}

// ── Sites ───────────────────────────────────────────────────────────

/** Each change saves at once (as the vault's policy call does). */
export function SitesDialog({ secret, onClose, onChanged }: { secret: VaultSecret; onClose: () => void; onChanged: () => void }): React.JSX.Element {
  const [hosts, setHosts] = useState<string[]>(secret.hosts)
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const id = useId()
  const run = async (body: { addHosts?: string[]; removeHosts?: string[] }, note: string): Promise<void> => {
    setBusy(true)
    setError(null)
    setSaved(null)
    try {
      const r = await vaultApi.policy({ id: secret.id, ...body })
      const next = Array.isArray(r?.secret?.hosts)
        ? r.secret.hosts
        : [...hosts.filter((h) => !(body.removeHosts ?? []).includes(h)), ...(body.addHosts ?? [])]
      setHosts(next)
      setDraft('')
      setSaved(note)
      onChanged()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  const add = (): void => {
    const list = draft
      .split(/[,\s]+/)
      .map((x) => x.trim())
      .filter(Boolean)
    if (list.length) void run({ addHosts: list }, `Added ${list.join(', ')}.`)
  }
  return (
    <Sheet
      title={`Sites for ${secret.name}`}
      intro="K2 sends it only to these sites. Each change saves at once; agents follow it from their next call."
      onClose={onClose}
      busy={busy}
      testId="vault-sites-panel"
      footer={
        <>
          <Button variant="accent" size="md" onClick={onClose} disabled={busy}>
            Done
          </Button>
        </>
      }
    >
      {hosts.length === 0 ? (
        <p className={MUTED}>No sites yet. Agents get it another way, or not at all, until you add one.</p>
      ) : (
        <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]">
          {hosts.map((h) => (
            <div key={h} className="flex items-center justify-between gap-4 px-4 py-2.5">
              <span className="font-mono type-row text-[var(--color-text-primary)] break-all">{siteLabel(h)}</span>
              <Button variant="quiet" disabled={busy} aria-label={`Remove ${siteLabel(h)}`} onClick={() => void run({ removeHosts: [h] }, `Removed ${siteLabel(h)}.`)}>
                Remove
              </Button>
            </div>
          ))}
        </Surface>
      )}
      <form
        onSubmit={(e) => {
          e.preventDefault()
          add()
        }}
      >
        <Field label="Add a site" hint="Like api.example.com. Separate several with commas." htmlFor={id}>
          <div className="flex items-center gap-2">
            <Input id={id} value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="api.example.com" className={`${FIELD} font-mono`} />
            <Button type="submit" className="flex-shrink-0" disabled={busy || !draft.trim()}>
              Add
            </Button>
          </div>
        </Field>
      </form>
      {saved && !error ? <SavedLine text={saved} /> : null}
      {error ? <FailedNote what="Couldn’t change the sites." detail={error} next="The sites are as they were. Try again." /> : null}
    </Sheet>
  )
}

/** A quiet "saved" line with a check. */
export function SavedLine({ text }: { text: string }): React.JSX.Element {
  return (
    <p role="status" className="flex items-start gap-1.5 type-hint text-[var(--color-status-ok-soft)]">
      <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 mt-0.5 flex-shrink-0">
        <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
      </svg>
      {text}
    </p>
  )
}

// ── How agents get it ───────────────────────────────────────────────

export function DeliveryDialog({ secret, onClose, onSaved }: { secret: VaultSecret; onClose: () => void; onSaved: (note: string) => void }): React.JSX.Element {
  const [delivery, setDelivery] = useState(secret.defaultDelivery === 'proxy' ? 'fetch' : secret.defaultDelivery)
  const [ceiling, setCeiling] = useState(secret.deliveryCeiling === 'proxy' ? 'fetch' : secret.deliveryCeiling)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const save = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await vaultApi.policy({ id: secret.id, delivery, ceiling })
      onSaved(`Saved how agents get ${secret.name}. It applies from their next call.`)
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <Sheet
      title={`How agents get ${secret.name}`}
      onClose={onClose}
      busy={busy}
      testId="vault-delivery-panel"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="accent" size="md" disabled={busy} onClick={() => void save()}>
            {busy ? 'Saving…' : 'Save'}
          </Button>
        </>
      }
    >
      <Pick label="Usually" hint="What an agent gets when it asks for it.">
        <SettingDropdown
          fullWidth
          menuAlign="left"
          ariaLabel="How agents get it"
          triggerData={{ 'data-testid': 'vault-delivery-select' }}
          value={delivery}
          options={deliveryOptions(withCurrent(deliveryChoices(secret.kind), delivery))}
          onChange={setDelivery}
        />
      </Pick>
      <Pick label="At most" hint="The most an agent may ever get, even when shared with it.">
        <SettingDropdown
          fullWidth
          menuAlign="left"
          ariaLabel="The most an agent may ever get"
          value={ceiling}
          options={deliveryOptions(withCurrent(ceilingChoices(secret.kind), ceiling))}
          onChange={setCeiling}
        />
      </Pick>
      {deliveryReadable(delivery) && <ReadableNote text="Agents can see this value." />}
      {error ? <FailedNote what="Couldn’t save." detail={error} next="Nothing changed. Try again." /> : null}
    </Sheet>
  )
}

// ── Share ───────────────────────────────────────────────────────────

export function ShareDialog({ secret, onClose, onSaved }: { secret: VaultSecret; onClose: () => void; onSaved: (note: string) => void }): React.JSX.Element {
  const formId = useId()
  const projects = useProjectsStore((s) => s.projects)
  const [to, setTo] = useState('ws:*')
  const [as, setAs] = useState(secret.defaultDelivery === 'proxy' ? 'fetch' : secret.defaultDelivery)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // VC-B2: a share they can read needs a confirm first, like reveal.
  const c = useVaultConfirm('grant-read', secret.id, secret.name)
  const targets = [{ value: 'ws:*', label: 'All my agents' }, ...projects.map((p) => ({ value: `ws:${p.id}`, label: p.handle || p.name }))]
  const toLabel = targets.find((t) => t.value === to)?.label ?? 'them'
  const grantWith = async (confirmToken?: string): Promise<void> => {
    await vaultApi.grant({ id: secret.id, to, as, ...(confirmToken ? { confirmToken } : {}) })
    onSaved(`${secret.name} is shared with ${to === 'ws:*' ? 'all your agents' : toLabel}.`)
  }
  const share = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      if (!deliveryReadable(as)) {
        await grantWith()
        return
      }
      const token = await c.begin()
      if (token) await grantWith(token)
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  const submitPassword = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const token = await c.withPassword()
      if (token) await grantWith(token)
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  const pw = c.ask.step === 'password'
  return (
    <Sheet
      title={`Share ${secret.name}`}
      intro="Let other agents use it. They get it the way you choose here."
      onClose={onClose}
      busy={busy}
      testId="vault-share-panel"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={pw ? c.reset : onClose} disabled={busy}>
            {pw ? 'Back' : 'Cancel'}
          </Button>
          <Button type="submit" form={formId} variant="accent" size="md" disabled={busy || (pw && !c.hasPw) || c.ask.step === 'none'}>
            {pw ? 'Confirm and share' : busy ? 'Sharing…' : 'Share'}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          if (pw) {
            if (c.hasPw) void submitPassword()
          } else void share()
        }}
      >
        <Pick label="Share with">
          <SettingDropdown fullWidth menuAlign="left" ariaLabel="Share with" value={to} options={targets} onChange={setTo} disabled={pw} />
        </Pick>
        <Pick label="How they get it">
          <SettingDropdown
            fullWidth
            menuAlign="left"
            ariaLabel="How they get it"
            triggerData={{ 'data-testid': 'vault-share-as' }}
            value={as}
            options={deliveryOptions(withCurrent(deliveryChoices(secret.kind), as))}
            onChange={setAs}
            disabled={pw}
          />
        </Pick>
        {deliveryReadable(as) && <ReadableNote text="They can see this value, so you confirm it’s you first." />}
        <ConfirmStep c={c} testId="vault-share-password" />
        {error ? <FailedNote what="Couldn’t share it." detail={error} next="Nothing was shared. Try again." /> : null}
      </form>
    </Sheet>
  )
}

// ── Delete ──────────────────────────────────────────────────────────

export function DeleteDialog({ secret, onClose, onDeleted }: { secret: VaultSecret; onClose: () => void; onDeleted: (note: string) => void }): React.JSX.Element {
  const formId = useId()
  const [typed, setTyped] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const id = useId()
  const ready = typed === secret.name && !busy
  const destroy = async (): Promise<void> => {
    if (!ready) return
    setBusy(true)
    setError(null)
    try {
      await vaultApi.destroy(secret.id)
      onDeleted(`${secret.name} is deleted from this server. Backups made before now keep it until they expire.`)
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <Sheet
      title={`Delete ${secret.name}?`}
      onClose={onClose}
      busy={busy}
      testId="vault-destroy-panel"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={formId} variant="danger-muted" size="md" disabled={!ready}>
            {busy ? 'Deleting…' : 'Delete secret'}
          </Button>
        </>
      }
    >
      <p className={MUTED}>
        Every version goes from this server, and agents that use it stop getting it. This can’t be undone; to stop agents for now, turn it off instead.
      </p>
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void destroy()
        }}
      >
        <Field label={`Type ${secret.name} to confirm`} htmlFor={id}>
          <Input
            id={id}
            autoFocus
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            aria-label="Type the name to confirm"
            autoComplete="off"
            spellCheck={false}
            className={`${FIELD} font-mono`}
          />
        </Field>
        {error ? <FailedNote what={`Couldn’t delete ${secret.name}.`} detail={error} next="It’s still in the vault. Try again." /> : null}
      </form>
    </Sheet>
  )
}

// ── History ─────────────────────────────────────────────────────────

export function HistoryDialog({ secret, onClose }: { secret: VaultSecret; onClose: () => void }): React.JSX.Element {
  const [rows, setRows] = useState<VaultAuditRow[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [nonce, setNonce] = useState(0)
  useEffect(() => {
    let live = true
    setError(null)
    vaultApi
      .show(secret.id)
      .then((r) => live && setRows(Array.isArray(r?.audit) ? r.audit : []))
      .catch((e) => live && setError(daemonErrorText(e)))
    return () => {
      live = false
    }
  }, [secret.id, nonce])
  return (
    <Sheet
      title={`History of ${secret.name}`}
      intro="Every save, change and use, newest first."
      onClose={onClose}
      testId="vault-history-panel"
      footer={
        <>
          {error ? (
            <Button variant="ghost" size="md" onClick={() => setNonce((n) => n + 1)}>
              Try again
            </Button>
          ) : null}
          <Button variant="accent" size="md" onClick={onClose}>
            Done
          </Button>
        </>
      }
    >
      <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]" data-testid="vault-history-facts">
        <div className="px-4 py-2.5">
          <p className="type-hint text-[var(--color-text-secondary)]">Where it came from</p>
          <p className={SETTING_ROW_LABEL}>
            {sourceWords(secret)}. It’s on version {secret.latestVersion}.
          </p>
        </div>
        <div className="px-4 py-2.5">
          <p className="type-hint text-[var(--color-text-secondary)]">What agents can do</p>
          <p className={SETTING_ROW_LABEL}>
            {secret.status === 'disabled'
              ? 'Nothing: it’s off until you turn it on.'
              : `They can ${agentUseWords(secret.defaultDelivery)}. Change it with How agents get it.`}
          </p>
        </div>
        {secret.ref ? <CopyLine label="Reference agents can use" value={secret.ref} testId="vault-ref" /> : null}
      </Surface>
      {error ? (
        <FailedNote what="Couldn’t load the history." detail={error} next="The secret is fine. Try again in a moment." />
      ) : rows === null ? (
        <Waiting text="Loading the history…" />
      ) : rows.length === 0 ? (
        <p className={MUTED}>Nothing yet.</p>
      ) : (
        <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]">
          {rows.map((r) => (
            <div key={r.seq} className="flex items-start justify-between gap-4 px-4 py-2.5">
              <div className="min-w-0">
                <p className={SETTING_ROW_LABEL}>
                  {auditActionWords(r.action)}
                  {r.count > 1 ? ` ${r.count} times` : ''}
                  {r.outcome && r.outcome !== 'ok' ? <span className="text-[var(--color-status-error-soft)]"> · {r.outcome}</span> : null}
                </p>
                <p className={SETTING_ROW_HINT}>
                  By {actorWords(r.actor)}
                  {r.consumer && r.consumer !== r.actor.split(':')[1] ? ` for ${r.consumer}` : ''}
                  {r.targetHost ? ` · ${r.targetHost}` : ''}
                </p>
              </div>
              <span className="type-hint text-[var(--color-text-secondary)] whitespace-nowrap">{whenWords(r.ts)}</span>
            </div>
          ))}
        </Surface>
      )}
    </Sheet>
  )
}

// ── Recovery key ────────────────────────────────────────────────────

type RecoveryStep =
  | { step: 'intro' }
  | { step: 'shown'; challenge: VaultRecoveryChallenge }
  | { step: 'check'; id: string; checkWords: number[] }
  | { step: 'saving'; id: string; checkWords: number[] }
  | { step: 'done' }

/** Set up the vault's recovery key (§7.4, VS2): confirm it's you, write
 *  down 24 words shown once, type three back. */
export function RecoveryDialog({ onClose, onConfirmed }: { onClose: () => void; onConfirmed: () => void }): React.JSX.Element {
  const c = useVaultConfirm('recovery-setup', undefined, 'recovery key')
  const [state, setState] = useState<RecoveryStep>({ step: 'intro' })
  const [error, setError] = useState<{ what: string; next: string; detail: string } | null>(null)
  const [typed, setTyped] = useState<string[]>(['', '', ''])
  const [busy, setBusy] = useState(false)
  const formId = useId()

  const show = async (token: string): Promise<void> => {
    const challenge = await vaultApi.recoveryShowOnce(token)
    setState({ step: 'shown', challenge })
  }
  const failStart = (e: unknown): void => {
    c.reset()
    setError({ what: 'Couldn’t start the recovery key.', next: 'Nothing changed. Try again.', detail: daemonErrorText(e) })
  }
  const start = async (): Promise<void> => {
    setError(null)
    try {
      const token = await c.begin()
      if (token) await show(token)
    } catch (e) {
      failStart(e)
    }
  }
  const submitPassword = async (): Promise<void> => {
    setError(null)
    setBusy(true)
    try {
      const token = await c.withPassword()
      if (token) await show(token)
    } catch (e) {
      failStart(e)
    } finally {
      setBusy(false)
    }
  }
  const submitWords = async (id: string, checkWords: number[]): Promise<void> => {
    setError(null)
    setState({ step: 'saving', id, checkWords })
    try {
      await vaultApi.recoveryConfirm(
        id,
        typed.map((w) => w.trim().toLowerCase()),
      )
      setTyped(['', '', ''])
      setState({ step: 'done' })
      onConfirmed()
    } catch (e) {
      // The daemon drops the phrase on a wrong answer: start again.
      setTyped(['', '', ''])
      setState({ step: 'intro' })
      setError({ what: 'Those words didn’t match.', next: 'Start again to get a new recovery key: the last one is gone.', detail: daemonErrorText(e) })
    }
  }

  const inFlight = busy || state.step === 'saving' || c.ask.step === 'asking' || c.ask.step === 'mac'
  const body = (): React.JSX.Element => {
    if (state.step === 'done') {
      return (
        <>
          <SavedLine text="The recovery key is set up." />
          <p className={MUTED}>Keep the words somewhere offline and safe. K2 won’t show them again.</p>
        </>
      )
    }
    if (state.step === 'shown') {
      return (
        <div className="space-y-4" data-testid="vault-recovery-phrase">
          <Callout tone="warn" className="type-hint">
            Write these 24 words down, in order, and keep them offline. K2 shows them once. Anyone with these words and a copy of the vault can read
            every secret.
          </Callout>
          <Surface chamfer material="tile" className="px-4 py-3">
            <ol className="grid grid-cols-3 sm:grid-cols-4 gap-x-4 gap-y-1.5 font-mono type-row text-[var(--color-text-primary)] select-none">
              {state.challenge.phrase.split(/\s+/).map((w, i) => (
                <li key={i} data-testid="vault-recovery-word">
                  <span className="text-[var(--color-text-secondary)]">{i + 1}.</span> {w}
                </li>
              ))}
            </ol>
          </Surface>
          <p className={MUTED}>There’s no copy button: anything on the clipboard, an agent could read.</p>
        </div>
      )
    }
    if (state.step === 'check' || state.step === 'saving') {
      const { id, checkWords } = state
      return (
        <form
          id={formId}
          className="space-y-4"
          data-testid="vault-recovery-check"
          onSubmit={(e) => {
            e.preventDefault()
            if (state.step === 'check' && !typed.some((w) => w.trim() === '')) void submitWords(id, checkWords)
          }}
        >
          <p className={MUTED}>Type back these words from your copy, to check it.</p>
          <div className="grid grid-cols-3 gap-3">
            {checkWords.map((n, i) => (
              <WordField
                key={n}
                n={n}
                value={typed[i] ?? ''}
                autoFocus={i === 0}
                disabled={state.step === 'saving'}
                onChange={(v) => setTyped((t) => t.map((x, j) => (j === i ? v : x)))}
              />
            ))}
          </div>
          {state.step === 'saving' ? <Waiting text="Checking…" /> : null}
        </form>
      )
    }
    const pw = c.ask.step === 'password'
    return (
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          if (pw) {
            if (c.hasPw && !busy) void submitPassword()
          } else if (c.ask.step === 'idle') void start()
        }}
      >
        {c.ask.step === 'idle' ? (
          <p className={MUTED}>
            If this server’s vault key is ever lost, these words are the only way back into your secrets. You confirm it’s you, write down 24 words,
            then type three of them back.
          </p>
        ) : null}
        <ConfirmStep c={c} testId="vault-recovery-password" other={{ text: 'Or set up the recovery key there instead:', command: 'k2 vault recovery show-once' }} />
        {error ? <FailedNote what={error.what} detail={error.detail} next={error.next} /> : null}
      </form>
    )
  }
  // The step's buttons, pinned under the body.
  const foot = (): React.JSX.Element | null => {
    if (state.step === 'done') {
      return (
        <Button variant="accent" size="md" onClick={onClose} autoFocus>
          Done
        </Button>
      )
    }
    if (state.step === 'shown') {
      const { challenge } = state
      return (
        <Button
          variant="accent"
          size="md"
          data-testid="vault-recovery-saved"
          onClick={() => setState({ step: 'check', id: challenge.id, checkWords: challenge.checkWords })}
        >
          I’ve written them down
        </Button>
      )
    }
    if (state.step === 'check' || state.step === 'saving') {
      return (
        <Button type="submit" form={formId} variant="accent" size="md" disabled={state.step === 'saving' || typed.some((w) => w.trim() === '')}>
          Confirm recovery key
        </Button>
      )
    }
    const pw = c.ask.step === 'password'
    return (
      <>
        <Button variant="ghost" size="md" onClick={pw ? c.reset : onClose} disabled={busy}>
          {pw ? 'Back' : 'Cancel'}
        </Button>
        {pw ? (
          <Button type="submit" form={formId} variant="accent" size="md" disabled={!c.hasPw || busy}>
            Confirm
          </Button>
        ) : c.ask.step === 'idle' ? (
          <Button type="submit" form={formId} variant="accent" size="md" data-testid="vault-recovery-start">
            {error ? 'Start again' : 'Continue'}
          </Button>
        ) : null}
      </>
    )
  }
  return (
    <Sheet title="Set up a recovery key" onClose={onClose} busy={inFlight} testId="vault-recovery-dialog" footer={foot()}>
      <div className="space-y-4" data-testid="vault-recovery-setup">
        {body()}
      </div>
    </Sheet>
  )
}

function WordField({ n, value, onChange, autoFocus, disabled }: { n: number; value: string; onChange: (v: string) => void; autoFocus: boolean; disabled: boolean }): React.JSX.Element {
  const id = useId()
  return (
    <div>
      <label htmlFor={id} className={LABEL}>
        Word {n}
      </label>
      <Input
        id={id}
        value={value}
        autoFocus={autoFocus}
        disabled={disabled}
        autoComplete="off"
        spellCheck={false}
        data-testid={`vault-recovery-word-${n}`}
        onChange={(e) => onChange(e.currentTarget.value)}
        className={`${FIELD} font-mono`}
      />
    </div>
  )
}
