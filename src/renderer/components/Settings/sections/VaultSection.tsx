// Settings → Access → Vault (prd-k2-vault-v1 §16.1, KV39; §18 copy).
//
// A thin client on `/cli/vault/*`. The daemon decides who sees what: a
// Member's list comes back filtered, and K2's own (`system`) secrets only
// come back to people allowed to see them, as metadata.
//
// Values: the only place a value is ever on screen is the reveal panel,
// after a confirm (the Mac's own sheet, or a password). It lives in that
// panel's state only, masked until "Show", and goes after 30 seconds or when
// the panel closes. Pasted values (add, rotate) are read from their field
// once on submit, sent as `valueB64` in the POST body, and the field is
// emptied. Nothing is stored, logged or kept in a store.
//
// VS1 is deferred for 0.45.2 (Rosson): this page talks to the daemon with
// the app's owner token like every other Settings page; no attested channel.
// Recovery-key setup (0.45.4): the owner token alone isn't a person (VS2),
// so the page first asks for a confirm (the Mac's sheet, or a password),
// then shows the 24 words once and has the person type back three. The
// words live in that panel's state only, are never copied to the
// clipboard, and are gone once the check step starts.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { useProjectsStore } from '@/stores/projects'
import { SquareRadio } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import { siteLabel } from '@/components/SessionView/SecretCard'
import {
  fromBase64,
  toBase64,
  vaultApi,
  type VaultAuditRow,
  type VaultRecoveryChallenge,
  type VaultPreset,
  type VaultRequest,
  type VaultSecret,
  type VaultStatus,
} from './vault-api'
import { AccessAuditLink } from '../shared/AccessAuditLink'

export const VAULT_MANIFEST: SettingEntry[] = [
  {
    id: 'vault.status',
    section: 'vault',
    label: 'Vault',
    description: 'Where the vault key lives, the recovery key, and what the vault protects on this server',
    keywords: ['vault', 'secret', 'secrets', 'key', 'recovery', 'keychain', 'tamper'],
  },
  {
    id: 'vault.secrets',
    section: 'vault',
    label: 'Secrets',
    description: 'Secrets your agents use: sites, how they get them, sharing, rotate, reveal, history',
    keywords: ['vault', 'secret', 'api key', 'token', 'password', 'rotate', 'reveal', 'share'],
  },
  {
    id: 'vault.requests',
    section: 'vault',
    label: 'Secret requests',
    description: 'Thread secret cards: waiting, saved, declined, expired',
    keywords: ['vault', 'secret card', 'thread', 'request'],
  },
]

/** How long a revealed value stays on screen. */
export const REVEAL_SHOW_MS = 30_000

// ── Copy ─────────────────────────────────────────────────────────────

const DELIVERY_LABEL: Record<string, string> = {
  fetch: 'K2 adds it to calls',
  proxy: 'K2 adds it to calls',
  file: 'a file the agent can read',
  env: 'an environment variable the agent can read',
  api: 'the agent can print it',
  none: 'not given to agents',
}

export function deliveryLabel(d: string): string {
  return DELIVERY_LABEL[d] ?? d
}

const KIND_LABEL: Record<string, string> = {
  api_key: 'API key',
  password: 'password',
  oauth_token: 'sign-in token',
  ssh_key: 'SSH key',
  tls_key: 'TLS key',
  dsn: 'database address',
  url: 'URL',
  generic: 'secret',
}

function kindLabel(k: string): string {
  return KIND_LABEL[k] ?? k.replace(/_/g, ' ')
}

/** Where a secret came from, in words. */
export function originLabel(s: Pick<VaultSecret, 'origin' | 'createdBy'>): string {
  if (s.origin.startsWith('thread:')) return 'Thread card'
  if (s.origin.startsWith('adopt:')) return 'moved in from older K2 storage'
  if (s.createdBy.startsWith('agent:')) return `agent ${s.createdBy.slice('agent:'.length)}`
  if (s.createdBy.startsWith('guest:')) return `guest ${s.createdBy.slice('guest:'.length)}`
  return 'a person'
}

// ── Grouping ─────────────────────────────────────────────────────────

export interface SecretGroup {
  key: string
  title: string
  rows: VaultSecret[]
}

/** Server, then each workspace (A–Z), then chats, then anything else. */
export function groupSecrets(secrets: readonly VaultSecret[]): SecretGroup[] {
  const server: VaultSecret[] = []
  const chats: VaultSecret[] = []
  const other: VaultSecret[] = []
  const byWs = new Map<string, VaultSecret[]>()
  for (const s of secrets) {
    if (s.namespace !== 'user') continue
    const kind = s.scope?.kind
    if (kind === 'server') server.push(s)
    else if (kind === 'session') chats.push(s)
    else if (kind === 'workspace') {
      const label = s.workspace || s.scope.id || 'workspace'
      const list = byWs.get(label) ?? []
      list.push(s)
      byWs.set(label, list)
    } else other.push(s)
  }
  const byName = (a: VaultSecret, b: VaultSecret) => a.name.localeCompare(b.name)
  const out: SecretGroup[] = []
  if (server.length) out.push({ key: 'server', title: 'Server (all my agents, or shared)', rows: server.sort(byName) })
  for (const ws of [...byWs.keys()].sort((a, b) => a.localeCompare(b))) {
    out.push({ key: `ws:${ws}`, title: ws, rows: byWs.get(ws)!.sort(byName) })
  }
  if (chats.length) out.push({ key: 'session', title: 'Chats', rows: chats.sort(byName) })
  if (other.length) out.push({ key: 'other', title: 'Other', rows: other.sort(byName) })
  return out
}

// ── Shared bits ──────────────────────────────────────────────────────

const btn =
  'px-2 py-1 text-[10px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-muted)] cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed'
const btnPrimary =
  'px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed'
const input =
  'w-full px-2 py-1 text-[12px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]'
const muted = 'text-[var(--color-text-muted)]'
const danger = 'text-[var(--color-danger,#e5484d)]'

function ErrorLine({ text }: { text: string | null }): React.JSX.Element | null {
  if (!text) return null
  return (
    <p className={`text-[11px] ${danger}`} role="alert">
      {text}
    </p>
  )
}

function CopyButton({ text, label = 'Copy' }: { text: string; label?: string }): React.JSX.Element {
  const [copied, setCopied] = useState(false)
  return (
    <button
      type="button"
      className={btn}
      onClick={() => {
        void navigator.clipboard?.writeText(text)
        setCopied(true)
        setTimeout(() => setCopied(false), 1500)
      }}
    >
      {copied ? 'Copied' : label}
    </button>
  )
}

/** A password field whose text never enters React state. */
function ValueField({
  fieldRef,
  label,
  onPresence,
}: {
  fieldRef: React.RefObject<HTMLInputElement | null>
  label: string
  onPresence: (has: boolean) => void
}): React.JSX.Element {
  return (
    <input
      ref={fieldRef}
      type="password"
      autoComplete="off"
      spellCheck={false}
      aria-label={label}
      placeholder="Paste the value"
      onChange={(e) => onPresence(e.currentTarget.value !== '')}
      className={input}
    />
  )
}

// ── Status ───────────────────────────────────────────────────────────

// ── Recovery key setup ──────────────────────────────────────────────

type RecoveryState =
  | { step: 'idle' }
  | { step: 'starting' }
  | { step: 'mac' }
  | { step: 'password'; method: string }
  | { step: 'shown'; challenge: VaultRecoveryChallenge }
  | { step: 'check'; id: string; checkWords: number[] }
  | { step: 'saving' }

/** Set up the vault's recovery key from the app (§7.4, VS2). */
export function RecoverySetup({ onConfirmed }: { onConfirmed: () => void }): React.JSX.Element {
  const [state, setState] = useState<RecoveryState>({ step: 'idle' })
  const [error, setError] = useState<string | null>(null)
  const pwRef = useRef<HTMLInputElement>(null)
  const [hasPw, setHasPw] = useState(false)
  const [typed, setTyped] = useState<string[]>(['', '', ''])

  const show = async (method: string, password?: string): Promise<void> => {
    const { token } = await vaultApi.confirmFinish({
      action: 'recovery-setup',
      method,
      label: 'recovery key',
      ...(password !== undefined ? { password } : {}),
    })
    const challenge = await vaultApi.recoveryShowOnce(token)
    setState({ step: 'shown', challenge })
  }

  const start = async (): Promise<void> => {
    setError(null)
    setState({ step: 'starting' })
    try {
      const { methods } = await vaultApi.confirmStart('recovery-setup')
      if (methods.includes('local-auth')) {
        setState({ step: 'mac' })
        await show('local-auth')
        return
      }
      const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
      if (pw) {
        setState({ step: 'password', method: pw })
        return
      }
      setState({ step: 'idle' })
      setError('No way to confirm here yet. From your own SSH login or a terminal outside K2: k2 vault recovery show-once.')
    } catch (e) {
      setState({ step: 'idle' })
      setError(daemonErrorText(e))
    }
  }

  const submitPassword = async (method: string): Promise<void> => {
    const el = pwRef.current
    if (!el) return
    const pw = el.value
    el.value = ''
    setHasPw(false)
    setError(null)
    try {
      await show(method, pw)
    } catch (e) {
      setState({ step: 'idle' })
      setError(daemonErrorText(e))
    }
  }

  const submitWords = async (id: string): Promise<void> => {
    setError(null)
    setState({ step: 'saving' })
    try {
      await vaultApi.recoveryConfirm(
        id,
        typed.map((w) => w.trim().toLowerCase()),
      )
      setTyped(['', '', ''])
      setState({ step: 'idle' })
      onConfirmed()
    } catch (e) {
      // The daemon drops the phrase on a wrong answer: start again.
      setTyped(['', '', ''])
      setState({ step: 'idle' })
      setError(`${daemonErrorText(e)} Start again to get a new recovery key.`)
    }
  }

  return (
    <div className="space-y-1.5" data-testid="vault-recovery-setup">
      {state.step === 'idle' && (
        <button type="button" className={btnPrimary} onClick={() => void start()} data-testid="vault-recovery-start">
          Set up recovery key
        </button>
      )}
      {state.step === 'starting' && <p className={`text-[11px] ${muted}`}>Asking the vault…</p>}
      {state.step === 'mac' && <p className={`text-[11px] ${muted}`}>Confirm on your Mac.</p>}
      {state.step === 'password' && (
        <form
          className="flex flex-col gap-1.5 max-w-sm"
          onSubmit={(e) => {
            e.preventDefault()
            void submitPassword(state.method)
          }}
        >
          <label className={`text-[11px] ${muted}`}>
            {state.method === 'connect-password' ? 'Your K2 Connect password' : 'Your vault password'}
            <input
              ref={pwRef}
              type="password"
              autoComplete="current-password"
              data-testid="vault-recovery-password"
              onChange={(e) => setHasPw(e.currentTarget.value !== '')}
              className={`${input} mt-1`}
            />
          </label>
          <div className="flex gap-1.5">
            <button type="submit" className={btn} disabled={!hasPw}>
              Confirm
            </button>
            <button type="button" className={btn} onClick={() => setState({ step: 'idle' })}>
              Cancel
            </button>
          </div>
        </form>
      )}
      {state.step === 'shown' && (
        <div className="border border-[var(--color-border)] bg-[var(--color-bg)] p-2 space-y-2" data-testid="vault-recovery-phrase">
          <p className="text-[11px] text-[var(--color-text-primary)]">
            Write these 24 words down, in order, and keep them offline. K2 shows them once and keeps only the public
            half. Anyone with these words and a copy of the vault can read every secret.
          </p>
          <ol className="grid grid-cols-3 sm:grid-cols-4 gap-x-3 gap-y-0.5 font-mono text-[11px] text-[var(--color-text-primary)] select-none">
            {state.challenge.phrase.split(/\s+/).map((w, i) => (
              <li key={i} data-testid="vault-recovery-word">
                <span className={muted}>{i + 1}.</span> {w}
              </li>
            ))}
          </ol>
          <div className="flex gap-1.5">
            <button
              type="button"
              className={btnPrimary}
              data-testid="vault-recovery-saved"
              onClick={() => setState({ step: 'check', id: state.challenge.id, checkWords: state.challenge.checkWords })}
            >
              I&rsquo;ve written it down
            </button>
          </div>
        </div>
      )}
      {(state.step === 'check' || state.step === 'saving') && (
        <form
          className="flex flex-col gap-1.5 max-w-sm"
          data-testid="vault-recovery-check"
          onSubmit={(e) => {
            e.preventDefault()
            if (state.step === 'check') void submitWords(state.id)
          }}
        >
          <p className={`text-[11px] ${muted}`}>Type back these words from your copy.</p>
          {state.step === 'check' &&
            state.checkWords.map((n, i) => (
              <label key={n} className={`text-[11px] ${muted}`}>
                Word {n}
                <input
                  value={typed[i] ?? ''}
                  autoComplete="off"
                  spellCheck={false}
                  data-testid={`vault-recovery-word-${n}`}
                  onChange={(e) => {
                    const v = e.currentTarget.value
                    setTyped((t) => t.map((x, j) => (j === i ? v : x)))
                  }}
                  className={`${input} mt-1`}
                />
              </label>
            ))}
          {state.step === 'saving' && <p className={`text-[11px] ${muted}`}>Checking…</p>}
          {state.step === 'check' && (
            <div className="flex gap-1.5">
              <button type="submit" className={btnPrimary} disabled={typed.some((w) => w.trim() === '')}>
                Confirm recovery key
              </button>
            </div>
          )}
        </form>
      )}
      <ErrorLine text={error} />
    </div>
  )
}

export function VaultStatusCard({
  status,
  onRecoveryConfirmed,
}: {
  status: VaultStatus
  onRecoveryConfirmed?: () => void
}): React.JSX.Element {
  if (!status.open) {
    return (
      <div className="border border-[var(--color-border)] p-3 space-y-1" data-testid="vault-status">
        <div className="text-[12px] font-medium text-[var(--color-text-primary)]">The vault isn&rsquo;t open</div>
        <p className={`text-[11px] ${muted}`}>{status.error?.hint || status.error?.code || 'K2 couldn’t open the vault on this server.'}</p>
      </div>
    )
  }
  const recoveryOk = status.recovery === 'confirmed'
  const tamperOk = !status.tamper || status.tamper === 'ok'
  return (
    <div className="space-y-2" data-testid="vault-status">
      {!recoveryOk && (
        <div
          role="alert"
          data-testid="vault-recovery-banner"
          className="border border-[var(--color-danger,#e5484d)] bg-[var(--color-danger,#e5484d)]/10 p-3 space-y-1"
        >
          <div className={`text-[12px] font-medium ${danger}`}>The vault has no confirmed recovery key</div>
          <p className="text-[11px] text-[var(--color-text-primary)]">
            {status.recoveryHint ||
              'Only you should do this, from your own SSH login or a terminal outside K2: k2 vault recovery show-once.'}
          </p>
          <RecoverySetup onConfirmed={() => onRecoveryConfirmed?.()} />
        </div>
      )}
      <div className="border border-[var(--color-border)] p-3 space-y-1.5">
        <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-[11px]">
          <dt className={muted}>Where the key lives</dt>
          <dd className="text-[var(--color-text-primary)]" data-testid="vault-key-home">
            {status.keyHomeLabel || status.keyHome || 'unknown'}
          </dd>
          <dt className={muted}>Recovery key</dt>
          <dd className={recoveryOk ? 'text-[var(--color-text-primary)]' : danger}>
            {recoveryOk
              ? `confirmed${status.recoveryConfirmedAt ? ` ${status.recoveryConfirmedAt.slice(0, 10)}` : ''}`
              : 'not confirmed'}
          </dd>
          <dt className={muted}>Rollback guard</dt>
          <dd className={status.rollback ? danger : 'text-[var(--color-text-primary)]'}>
            {status.rollbackGuard || 'off'}
            {status.rollback ? ' · the vault file looks older than expected' : ''}
          </dd>
          <dt className={muted}>Tamper check</dt>
          <dd className={tamperOk ? 'text-[var(--color-text-primary)]' : danger} data-testid="vault-tamper">
            {tamperOk ? 'ok' : status.tamper}
          </dd>
          <dt className={muted}>Vault password</dt>
          <dd className="text-[var(--color-text-primary)]">{status.confirmPassword ? 'set' : 'not set'}</dd>
          {status.counts && (
            <>
              <dt className={muted}>Secrets</dt>
              <dd className="text-[var(--color-text-primary)]">
                {status.counts.user} yours · {status.counts.system} K2&rsquo;s own · {status.counts.pendingRequests} waiting
                {status.counts.rotateDue ? ` · ${status.counts.rotateDue} due for rotation` : ''}
              </dd>
            </>
          )}
        </dl>
        {status.honest && (
          <p className="text-[11px] text-[var(--color-text-secondary)] leading-relaxed" data-testid="vault-honest">
            {status.honest}
          </p>
        )}
        <p className={`text-[10px] ${muted} leading-relaxed`}>
          For now, an agent that gets hold of this server&rsquo;s owner token can change a secret&rsquo;s sites or how
          it&rsquo;s given out, as you can. Each secret&rsquo;s history records it.
        </p>
        {status.adopted && status.adopted.length > 0 && (
          <p className={`text-[10px] ${muted}`} data-testid="vault-adopted">
            Moved in from older storage:{' '}
            {status.adopted.map((a) => `${a.store} (${a.moved} moved${a.kept ? `, ${a.kept} kept` : ''})`).join(', ')}.
            {status.legacyCopies ? ` ${status.legacyCopies[0].toUpperCase()}${status.legacyCopies.slice(1)}.` : ''}
          </p>
        )}
      </div>
    </div>
  )
}

// ── Reveal ───────────────────────────────────────────────────────────

type RevealState =
  | { step: 'idle' }
  | { step: 'starting' }
  | { step: 'mac' }
  | { step: 'password'; method: string }
  | { step: 'shown'; value: string; version: number }

/** Reveal after a confirm. The value lives here only, for 30 seconds. */
export function RevealPanel({ secret }: { secret: VaultSecret }): React.JSX.Element {
  const [state, setState] = useState<RevealState>({ step: 'idle' })
  const [visible, setVisible] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const pwRef = useRef<HTMLInputElement>(null)
  const [hasPw, setHasPw] = useState(false)

  const hide = useCallback(() => {
    setState({ step: 'idle' })
    setVisible(false)
  }, [])

  useEffect(() => {
    if (state.step !== 'shown') return
    const t = setTimeout(hide, REVEAL_SHOW_MS)
    return () => clearTimeout(t)
  }, [state, hide])

  const finish = async (method: string, password?: string): Promise<void> => {
    const { token } = await vaultApi.confirmFinish({
      action: 'reveal',
      secretId: secret.id,
      method,
      label: secret.name,
      ...(password !== undefined ? { password } : {}),
    })
    const r = await vaultApi.reveal(secret.id, token)
    setState({ step: 'shown', value: fromBase64(r.valueB64), version: r.version })
    setVisible(false)
  }

  const start = async (): Promise<void> => {
    setError(null)
    setState({ step: 'starting' })
    try {
      const { methods } = await vaultApi.confirmStart('reveal', secret.id)
      if (methods.includes('local-auth')) {
        setState({ step: 'mac' })
        await finish('local-auth')
        return
      }
      const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
      if (pw) {
        setState({ step: 'password', method: pw })
        return
      }
      setState({ step: 'idle' })
      setError('No way to confirm here yet. Set a vault password from a terminal outside K2: k2 vault confirm-password set.')
    } catch (e) {
      setState({ step: 'idle' })
      setError(daemonErrorText(e))
    }
  }

  const submitPassword = async (method: string): Promise<void> => {
    const el = pwRef.current
    if (!el) return
    const pw = el.value
    el.value = ''
    setHasPw(false)
    setError(null)
    try {
      await finish(method, pw)
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }

  return (
    <div className="space-y-1.5" data-testid="vault-reveal">
      {state.step === 'idle' && (
        <button type="button" className={btn} onClick={() => void start()} data-testid="vault-reveal-start">
          Reveal
        </button>
      )}
      {state.step === 'starting' && <p className={`text-[11px] ${muted}`}>Asking the vault…</p>}
      {state.step === 'mac' && <p className={`text-[11px] ${muted}`}>Confirm on your Mac.</p>}
      {state.step === 'password' && (
        <form
          className="flex flex-col gap-1.5 max-w-sm"
          onSubmit={(e) => {
            e.preventDefault()
            void submitPassword(state.method)
          }}
        >
          <label className={`text-[11px] ${muted}`}>
            {state.method === 'connect-password' ? 'Your K2 Connect password' : 'Your vault password'}
            <input
              ref={pwRef}
              type="password"
              autoComplete="current-password"
              data-testid="vault-reveal-password"
              onChange={(e) => setHasPw(e.currentTarget.value !== '')}
              className={`${input} mt-1`}
            />
          </label>
          <div className="flex gap-1.5">
            <button type="submit" className={btn} disabled={!hasPw}>
              Confirm
            </button>
            <button type="button" className={btn} onClick={hide}>
              Cancel
            </button>
          </div>
        </form>
      )}
      {state.step === 'shown' && (
        <div className="border border-[var(--color-border)] p-2 space-y-1.5" data-testid="vault-reveal-panel">
          <div className="font-mono text-[11px] text-[var(--color-text-primary)] break-all select-all" data-testid="vault-reveal-value">
            {visible ? state.value : '•'.repeat(Math.min(24, Math.max(8, state.value.length)))}
          </div>
          <div className="flex gap-1.5 items-center">
            <button type="button" className={btn} onClick={() => setVisible((v) => !v)}>
              {visible ? 'Hide' : 'Show'}
            </button>
            <CopyButton text={state.value} />
            <button type="button" className={btn} onClick={hide}>
              Done
            </button>
            <span className={`text-[10px] ${muted}`}>Version {state.version}. Hides after 30 seconds.</span>
          </div>
        </div>
      )}
      <ErrorLine text={error} />
    </div>
  )
}

// ── Row actions ──────────────────────────────────────────────────────

type Mode = null | 'sites' | 'delivery' | 'share' | 'rotate' | 'destroy' | 'history' | 'reveal'

function SitesPanel({ secret, onDone }: { secret: VaultSecret; onDone: () => void }): React.JSX.Element {
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const run = async (body: { addHosts?: string[]; removeHosts?: string[] }) => {
    setBusy(true)
    setError(null)
    try {
      await vaultApi.policy({ id: secret.id, ...body })
      setDraft('')
      onDone()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="space-y-1.5" data-testid="vault-sites-panel">
      {secret.hosts.length === 0 && <p className={`text-[11px] ${muted}`}>No sites yet.</p>}
      {secret.hosts.map((h) => (
        <div key={h} className="flex items-center gap-2 text-[11px]">
          <span className="font-mono text-[var(--color-text-primary)]">{siteLabel(h)}</span>
          <button type="button" className={btn} disabled={busy} onClick={() => void run({ removeHosts: [h] })}>
            Remove
          </button>
        </div>
      ))}
      <div className="flex gap-1.5 max-w-md">
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="Add a site, like api.example.com"
          className={input}
          aria-label="Add a site"
        />
        <button
          type="button"
          className={btn}
          disabled={busy || !draft.trim()}
          onClick={() =>
            void run({
              addHosts: draft
                .split(/[,\s]+/)
                .map((x) => x.trim())
                .filter(Boolean),
            })
          }
        >
          Add
        </button>
      </div>
      <ErrorLine text={error} />
    </div>
  )
}

const DELIVERY_CHOICES = ['fetch', 'file', 'env'] as const
const CEILING_CHOICES = ['fetch', 'file', 'env', 'api'] as const

function DeliveryPanel({ secret, onDone }: { secret: VaultSecret; onDone: () => void }): React.JSX.Element {
  const [delivery, setDelivery] = useState(secret.defaultDelivery === 'proxy' ? 'fetch' : secret.defaultDelivery)
  const [ceiling, setCeiling] = useState(secret.deliveryCeiling === 'proxy' ? 'fetch' : secret.deliveryCeiling)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const save = async () => {
    setBusy(true)
    setError(null)
    try {
      await vaultApi.policy({ id: secret.id, delivery, ceiling })
      onDone()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="space-y-1.5 max-w-md" data-testid="vault-delivery-panel">
      <label className={`block text-[11px] ${muted}`}>
        How agents get it
        <select value={delivery} onChange={(e) => setDelivery(e.target.value)} className={`${input} mt-1`}>
          {DELIVERY_CHOICES.map((d) => (
            <option key={d} value={d}>
              {deliveryLabel(d)}
            </option>
          ))}
        </select>
      </label>
      <label className={`block text-[11px] ${muted}`}>
        The most an agent may ever get
        <select value={ceiling} onChange={(e) => setCeiling(e.target.value)} className={`${input} mt-1`}>
          {CEILING_CHOICES.map((d) => (
            <option key={d} value={d}>
              {deliveryLabel(d)}
            </option>
          ))}
        </select>
      </label>
      {delivery !== 'fetch' && <p className={`text-[11px] ${muted}`}>Agents can see this value.</p>}
      <button type="button" className={btn} disabled={busy} onClick={() => void save()}>
        Save
      </button>
      <ErrorLine text={error} />
    </div>
  )
}

function SharePanel({ secret, onDone }: { secret: VaultSecret; onDone: () => void }): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const [to, setTo] = useState('ws:*')
  const [as, setAs] = useState(secret.defaultDelivery === 'proxy' ? 'fetch' : secret.defaultDelivery)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const share = async () => {
    setBusy(true)
    setError(null)
    try {
      await vaultApi.grant({ id: secret.id, to, as })
      onDone()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="space-y-1.5 max-w-md" data-testid="vault-share-panel">
      <label className={`block text-[11px] ${muted}`}>
        Share with
        <select value={to} onChange={(e) => setTo(e.target.value)} className={`${input} mt-1`}>
          <option value="ws:*">all my agents</option>
          {projects.map((p) => (
            <option key={p.id} value={`ws:${p.id}`}>
              {p.handle || p.name}
            </option>
          ))}
        </select>
      </label>
      <label className={`block text-[11px] ${muted}`}>
        How they get it
        <select value={as} onChange={(e) => setAs(e.target.value)} className={`${input} mt-1`}>
          {DELIVERY_CHOICES.map((d) => (
            <option key={d} value={d}>
              {deliveryLabel(d)}
            </option>
          ))}
        </select>
      </label>
      <button type="button" className={btn} disabled={busy} onClick={() => void share()}>
        Share
      </button>
      <ErrorLine text={error} />
    </div>
  )
}

function RotatePanel({ secret, onDone }: { secret: VaultSecret; onDone: () => void }): React.JSX.Element {
  const fieldRef = useRef<HTMLInputElement>(null)
  const [has, setHas] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const rotate = async () => {
    const el = fieldRef.current
    if (!el || !el.value) return
    setBusy(true)
    setError(null)
    try {
      const valueB64 = toBase64(el.value)
      el.value = ''
      setHas(false)
      await vaultApi.rotate({ id: secret.id, valueB64 })
      onDone()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="space-y-1.5 max-w-md" data-testid="vault-rotate-panel">
      <p className={`text-[11px] ${muted}`}>Paste the new value from {secret.provider || 'the provider'}. Agents use it from their next call.</p>
      <ValueField fieldRef={fieldRef} label={`New value for ${secret.name}`} onPresence={setHas} />
      <button type="button" className={btn} disabled={busy || !has} onClick={() => void rotate()}>
        Rotate
      </button>
      <ErrorLine text={error} />
    </div>
  )
}

function DestroyPanel({ secret, onDone }: { secret: VaultSecret; onDone: () => void }): React.JSX.Element {
  const [typed, setTyped] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const destroy = async () => {
    setBusy(true)
    setError(null)
    try {
      await vaultApi.destroy(secret.id)
      onDone()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="space-y-1.5 max-w-md" data-testid="vault-destroy-panel">
      <p className="text-[11px] text-[var(--color-text-primary)]">
        Destroy removes every version from this server. Earlier backups keep it until they expire.
      </p>
      <label className={`block text-[11px] ${muted}`}>
        Type {secret.name} to confirm
        <input value={typed} onChange={(e) => setTyped(e.target.value)} className={`${input} mt-1`} aria-label="Type the name to confirm" />
      </label>
      <button
        type="button"
        className={`${btn} ${danger}`}
        disabled={busy || typed !== secret.name}
        onClick={() => void destroy()}
      >
        Destroy
      </button>
      <ErrorLine text={error} />
    </div>
  )
}

function HistoryPanel({ secret }: { secret: VaultSecret }): React.JSX.Element {
  const [rows, setRows] = useState<VaultAuditRow[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let live = true
    vaultApi
      .show(secret.id)
      .then((r) => live && setRows(Array.isArray(r?.audit) ? r.audit : []))
      .catch((e) => live && setError(daemonErrorText(e)))
    return () => {
      live = false
    }
  }, [secret.id])
  return (
    <div className="space-y-1" data-testid="vault-history-panel">
      <ErrorLine text={error} />
      {rows === null && !error && <p className={`text-[11px] ${muted}`}>Loading…</p>}
      {rows !== null && rows.length === 0 && <p className={`text-[11px] ${muted}`}>Nothing yet.</p>}
      {rows?.map((r) => (
        <div key={r.seq} className="text-[10px] font-mono text-[var(--color-text-secondary)] break-all">
          {r.ts} · {r.action} · {r.actor}
          {r.consumer ? ` · ${r.consumer}` : ''}
          {r.targetHost ? ` · ${r.targetHost}` : ''} · {r.outcome}
          {r.count > 1 ? ` ×${r.count}` : ''}
        </div>
      ))}
    </div>
  )
}

export function SecretRow({ secret, onChanged }: { secret: VaultSecret; onChanged: () => void }): React.JSX.Element {
  const [mode, setMode] = useState<Mode>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const disabled = secret.status === 'disabled'
  const done = () => {
    setMode(null)
    onChanged()
  }
  const toggleStatus = async () => {
    setBusy(true)
    setError(null)
    try {
      if (disabled) await vaultApi.enable(secret.id)
      else await vaultApi.disable(secret.id)
      onChanged()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  const pick = (m: Exclude<Mode, null>) => setMode((cur) => (cur === m ? null : m))
  const sites = secret.hostsDisplay ?? secret.hosts.map((origin) => ({ origin, unicode: '', mixedScript: false }))
  return (
    <div className="p-3 space-y-1.5" data-testid="vault-secret-row" data-secret={secret.name}>
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-[12px] font-mono font-medium text-[var(--color-text-primary)]">{secret.name}</span>
        <span className={`text-[10px] ${muted}`}>{kindLabel(secret.kind)}</span>
        <span className={`text-[10px] ${muted}`}>v{secret.latestVersion}</span>
        {disabled && <span className="text-[10px] px-1 border border-[var(--color-border)] text-[var(--color-text-secondary)]">disabled</span>}
        {secret.agentCanSee && (
          <span
            className="text-[10px] px-1 border border-[var(--color-status-warning-soft,var(--color-border))] text-[var(--color-text-secondary)]"
            data-testid="vault-agent-can-see"
          >
            agent can see this
          </span>
        )}
      </div>
      <div className="text-[11px] text-[var(--color-text-secondary)] space-y-0.5">
        <div>
          Sites:{' '}
          {sites.length === 0
            ? 'none'
            : sites.map((h, i) => (
                <span key={h.origin}>
                  {i > 0 ? ', ' : ''}
                  <span className="font-mono">{siteLabel(h.origin)}</span>
                  {h.mixedScript && (
                    <span className={danger} data-testid="vault-mixed-script">
                      {' '}
                      (mixed scripts{h.unicode ? `; reads as ${h.unicode}` : ''})
                    </span>
                  )}
                </span>
              ))}
        </div>
        <div>How agents get it: {deliveryLabel(secret.defaultDelivery)}</div>
        <div>
          From {originLabel(secret)}
          {secret.expiresAt ? ` · until ${secret.expiresAt.slice(0, 16).replace('T', ' ')}` : ''}
        </div>
        {secret.ref && (
          <div className="flex items-center gap-2">
            <span className="font-mono text-[10px] text-[var(--color-text-primary)] select-all">{secret.ref}</span>
            <CopyButton text={secret.ref} label="Copy reference" />
          </div>
        )}
      </div>
      <div className="flex flex-wrap gap-1.5">
        <button type="button" className={btn} onClick={() => pick('sites')}>
          Edit sites
        </button>
        <button type="button" className={btn} onClick={() => pick('delivery')}>
          Change delivery
        </button>
        <button type="button" className={btn} onClick={() => pick('share')}>
          Share
        </button>
        <button type="button" className={btn} onClick={() => pick('rotate')}>
          Rotate
        </button>
        <button type="button" className={btn} disabled={busy} onClick={() => void toggleStatus()}>
          {disabled ? 'Enable' : 'Disable'}
        </button>
        <button type="button" className={btn} onClick={() => pick('destroy')}>
          Destroy
        </button>
        <button type="button" className={btn} onClick={() => pick('history')}>
          History
        </button>
        <button type="button" className={btn} onClick={() => pick('reveal')} data-testid="vault-row-reveal">
          Reveal
        </button>
      </div>
      <ErrorLine text={error} />
      {mode === 'sites' && <SitesPanel secret={secret} onDone={done} />}
      {mode === 'delivery' && <DeliveryPanel secret={secret} onDone={done} />}
      {mode === 'share' && <SharePanel secret={secret} onDone={done} />}
      {mode === 'rotate' && <RotatePanel secret={secret} onDone={done} />}
      {mode === 'destroy' && <DestroyPanel secret={secret} onDone={done} />}
      {mode === 'history' && <HistoryPanel secret={secret} />}
      {mode === 'reveal' && <RevealPanel secret={secret} />}
    </div>
  )
}

/** K2's own secrets: metadata only, never actions or reveal. */
function SystemRow({ secret }: { secret: VaultSecret }): React.JSX.Element {
  return (
    <div className="px-3 py-2 text-[11px] flex flex-wrap gap-x-3" data-testid="vault-system-row">
      <span className="font-mono text-[var(--color-text-primary)]">{secret.name}</span>
      <span className={muted}>{kindLabel(secret.kind)}</span>
      <span className={muted}>v{secret.latestVersion}</span>
      <span className={muted}>{secret.status}</span>
    </div>
  )
}

// ── Add secret ───────────────────────────────────────────────────────

function AddSecretForm({ presets, onDone, onCancel }: { presets: VaultPreset[]; onDone: () => void; onCancel: () => void }): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const [name, setName] = useState('')
  const [scope, setScope] = useState<'workspace' | 'server'>('workspace')
  const [workspace, setWorkspace] = useState(projects[0]?.id ?? '')
  const [preset, setPreset] = useState('')
  const [sites, setSites] = useState('')
  const [delivery, setDelivery] = useState<(typeof DELIVERY_CHOICES)[number]>('fetch')
  const [kind, setKind] = useState('api_key')
  const fieldRef = useRef<HTMLInputElement>(null)
  const [has, setHas] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const presetRow = presets.find((p) => p.id === preset) ?? null
  const presetSites = (presetRow?.hosts ?? []).map(siteLabel).join(', ')
  const siteList = sites
    .split(/[,\s]+/)
    .map((s) => s.trim())
    .filter(Boolean)
  const needsSite = delivery === 'fetch' && siteList.length === 0
  const canSave = has && name.trim() !== '' && !needsSite && !busy && (scope === 'server' || workspace !== '')

  const save = async () => {
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
      onDone()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="border border-[var(--color-border)] p-3 space-y-2 max-w-lg" data-testid="vault-add-form">
      <h3 className="text-[12px] font-medium text-[var(--color-text-primary)]">Add a secret</h3>
      <label className={`block text-[11px] ${muted}`}>
        Name
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="STRIPE_KEY" className={`${input} mt-1 font-mono`} />
      </label>
      <fieldset className="text-[11px] space-y-1">
        <legend className={muted}>Keep it for</legend>
        <label className="flex items-center gap-1.5 text-[var(--color-text-secondary)] cursor-pointer">
          <SquareRadio checked={scope === 'workspace'} onChange={() => setScope('workspace')} />
          one workspace
        </label>
        {scope === 'workspace' && (
          <select value={workspace} onChange={(e) => setWorkspace(e.target.value)} className={input} aria-label="Workspace">
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.handle || p.name}
              </option>
            ))}
          </select>
        )}
        <label className="flex items-center gap-1.5 text-[var(--color-text-secondary)] cursor-pointer">
          <SquareRadio checked={scope === 'server'} onChange={() => setScope('server')} />
          all my agents
        </label>
      </fieldset>
      <label className={`block text-[11px] ${muted}`}>
        Kind
        <select value={kind} onChange={(e) => setKind(e.target.value)} className={`${input} mt-1`}>
          <option value="api_key">API key</option>
          <option value="password">password</option>
          <option value="oauth_token">sign-in token</option>
          <option value="generic">other</option>
        </select>
      </label>
      {presets.length > 0 && (
        <label className={`block text-[11px] ${muted}`}>
          Service
          <select
            value={preset}
            onChange={(e) => {
              const id = e.target.value
              setPreset(id)
              const p = presets.find((x) => x.id === id)
              setSites((p?.hosts ?? []).map(siteLabel).join(', '))
            }}
            className={`${input} mt-1`}
            data-testid="vault-add-preset"
          >
            <option value="">None (I&rsquo;ll set the sites)</option>
            {presets.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
          </select>
        </label>
      )}
      <label className={`block text-[11px] ${muted}`}>
        Only send it to
        <input
          value={sites}
          onChange={(e) => setSites(e.target.value)}
          placeholder="api.example.com"
          className={`${input} mt-1 font-mono`}
          data-testid="vault-add-sites"
        />
      </label>
      <label className={`block text-[11px] ${muted}`}>
        How agents get it
        <select
          value={delivery}
          onChange={(e) => setDelivery(e.target.value as (typeof DELIVERY_CHOICES)[number])}
          className={`${input} mt-1`}
        >
          {DELIVERY_CHOICES.map((d) => (
            <option key={d} value={d}>
              {deliveryLabel(d)}
            </option>
          ))}
        </select>
      </label>
      {delivery !== 'fetch' && <p className={`text-[11px] ${muted}`}>Agents can see this value.</p>}
      {needsSite && <p className={`text-[11px] ${danger}`}>Add a site, or let agents read it.</p>}
      <ValueField fieldRef={fieldRef} label="Value" onPresence={setHas} />
      <p className={`text-[10px] ${muted}`}>
        A key can still do anything its service allows it to. Use a restricted key if you can.
      </p>
      <div className="flex gap-1.5">
        <button type="button" className={btnPrimary} disabled={!canSave} onClick={() => void save()} data-testid="vault-add-save">
          Save
        </button>
        <button type="button" className={btn} onClick={onCancel}>
          Cancel
        </button>
      </div>
      <ErrorLine text={error} />
    </div>
  )
}

// ── Requests ─────────────────────────────────────────────────────────

const REQUEST_STATUS: Record<string, string> = { pending: 'waiting', set: 'saved', void: 'declined', expired: 'expired' }

function RequestsList(): React.JSX.Element {
  const [rows, setRows] = useState<VaultRequest[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let live = true
    vaultApi
      .requests()
      .then((r) => live && setRows(r))
      .catch((e) => live && setError(daemonErrorText(e)))
    return () => {
      live = false
    }
  }, [])
  return (
    <div className="space-y-1" data-testid="vault-requests">
      <ErrorLine text={error} />
      {rows === null && !error && <p className={`text-[11px] ${muted}`}>Loading…</p>}
      {rows !== null && rows.length === 0 && <p className={`text-[11px] ${muted}`}>No secret cards yet.</p>}
      {rows !== null && rows.length > 0 && (
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          {rows.map((r) => (
            <div key={r.cardId} className="p-2 text-[11px] space-y-0.5" data-testid="vault-request-row">
              <div className="flex flex-wrap gap-x-3">
                <span className="font-mono text-[var(--color-text-primary)]">{r.name}</span>
                <span className="text-[var(--color-text-secondary)]">asked by {r.requestedBy}</span>
                <span className={muted}>{REQUEST_STATUS[r.status] ?? r.status}</span>
                <span className={muted}>{r.createdAt.slice(0, 16).replace('T', ' ')}</span>
              </div>
              {r.prompt && <div className={`${muted} whitespace-pre-wrap break-words`}>{r.prompt}</div>}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

// ── Page ─────────────────────────────────────────────────────────────

export function VaultSection(): React.JSX.Element {
  const [tab, setTab] = useState<'secrets' | 'requests'>('secrets')
  const [status, setStatus] = useState<VaultStatus | null>(null)
  const [secrets, setSecrets] = useState<VaultSecret[]>([])
  const [system, setSystem] = useState<VaultSecret[]>([])
  const [presets, setPresets] = useState<VaultPreset[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loaded, setLoaded] = useState(false)
  const [adding, setAdding] = useState(false)

  const refresh = useCallback(async () => {
    const [st, list, sys, pre] = await Promise.allSettled([
      vaultApi.status(),
      vaultApi.list(),
      vaultApi.list({ system: true }),
      vaultApi.presets(),
    ])
    if (st.status === 'fulfilled') setStatus(st.value)
    if (list.status === 'fulfilled') {
      setSecrets(list.value)
      setError(null)
    } else {
      setError(daemonErrorText(list.reason))
    }
    // K2's own secrets only when the daemon hands them back (it filters).
    setSystem(sys.status === 'fulfilled' ? sys.value.filter((s) => s.namespace === 'system') : [])
    if (pre.status === 'fulfilled') setPresets(pre.value)
    setLoaded(true)
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const groups = useMemo(() => groupSecrets(secrets), [secrets])
  const vaultClosed = status !== null && !status.open

  return (
    <div className="max-w-4xl space-y-4" data-settings-id="vault.status">
      <div>
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">Vault</h2>
        <p className={`text-[11px] ${muted} mt-1 max-w-2xl`}>
          Secrets your agents use: API keys, passwords, tokens. Each one is only sent to the sites you pick, and every
          use is logged.
        </p>
        <AccessAuditLink className="mt-1" />
      </div>

      {status && <VaultStatusCard status={status} onRecoveryConfirmed={() => void refresh()} />}

      <div className="flex gap-1 border-b border-[var(--color-border)]" role="tablist">
        {(['secrets', 'requests'] as const).map((t) => (
          <button
            key={t}
            type="button"
            role="tab"
            aria-selected={tab === t}
            onClick={() => setTab(t)}
            data-settings-id={t === 'secrets' ? 'vault.secrets' : 'vault.requests'}
            className={`px-3 py-1.5 text-[11px] cursor-pointer -mb-px border-b-2 ${
              tab === t
                ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                : 'border-transparent text-[var(--color-text-muted)]'
            }`}
          >
            {t === 'secrets' ? 'Secrets' : 'Requests'}
          </button>
        ))}
      </div>

      {tab === 'requests' && <RequestsList />}

      {tab === 'secrets' && (
        <div className="space-y-3">
          <div className="flex items-center gap-2">
            <button
              type="button"
              className={btnPrimary}
              disabled={vaultClosed}
              onClick={() => setAdding(true)}
              data-testid="vault-add"
            >
              Add secret
            </button>
            <button type="button" className={btn} onClick={() => void refresh()}>
              Refresh
            </button>
          </div>
          {adding && (
            <AddSecretForm
              presets={presets}
              onCancel={() => setAdding(false)}
              onDone={() => {
                setAdding(false)
                void refresh()
              }}
            />
          )}
          <ErrorLine text={error} />
          {loaded && !error && groups.length === 0 && <p className={`text-[11px] ${muted}`}>No secrets yet.</p>}
          {groups.map((g) => (
            <div key={g.key} className="space-y-1" data-testid="vault-group" data-group={g.key}>
              <h3 className={`text-[11px] font-medium ${muted}`}>{g.title}</h3>
              <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
                {g.rows.map((s) => (
                  <SecretRow key={s.id} secret={s} onChanged={() => void refresh()} />
                ))}
              </div>
            </div>
          ))}
          {system.length > 0 && (
            <details className="border border-[var(--color-border)]" data-testid="vault-system">
              <summary className={`px-3 py-2 text-[11px] ${muted} cursor-pointer`}>
                K2&rsquo;s own secrets ({system.length})
              </summary>
              <p className={`px-3 text-[10px] ${muted}`}>K2 uses these itself. This page shows their names only.</p>
              <div className="divide-y divide-[var(--color-border)]">
                {system.map((s) => (
                  <SystemRow key={s.id} secret={s} />
                ))}
              </div>
            </details>
          )}
        </div>
      )}
    </div>
  )
}
