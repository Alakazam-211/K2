import React from 'react'

import { useCloneToDialogStore } from '../../stores/clone-to-dialog'
import {
  anyCopy,
  canCopyRow,
  MEMBER_NO_COPY_LINE,
  THREAD_SINCE_OPTIONS,
  threadSummaryLine,
  type SecretChoice,
} from '../../lib/clone-options'

// Options-phase blocks of the Clone dialog (prd-clone-carries-threads-and-
// vault-v1 §4.1): "Bring the Thread" (D3, ticked by default, always shown),
// the per-secret step (names only; Copy needs an Owner/Admin, D1), the
// `.env` files option (D4, needs a person confirm) and the password field a
// confirm asks for. All state lives in the clone-to-dialog store.

function Tick({ checked, onChange, children, testId, disabled }: {
  checked: boolean
  onChange: (v: boolean) => void
  children: React.ReactNode
  testId?: string
  disabled?: boolean
}): React.JSX.Element {
  return (
    <label className="flex items-start gap-2 cursor-pointer select-none no-drag">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        data-testid={testId}
        onChange={(e) => onChange(e.target.checked)}
        className="peer sr-only"
      />
      <span
        aria-hidden="true"
        className="mt-0.5 w-3.5 h-3.5 flex-shrink-0 flex items-center justify-center border transition-colors border-[var(--color-border)] bg-[var(--color-bg-elevated)] peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)] peer-focus-visible:ring-1 peer-focus-visible:ring-[var(--color-accent)]"
      >
        {checked && (
          <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M2.5 6.5 L5 9 L9.5 3.5" />
          </svg>
        )}
      </span>
      <span className="text-xs text-[var(--color-text-secondary)] leading-relaxed">{children}</span>
    </label>
  )
}

const muted = 'text-[11px] text-[var(--color-text-muted)] leading-relaxed'

export function ThreadBlock(): React.JSX.Element {
  const bring = useCloneToDialogStore((s) => s.bringThread)
  const setBring = useCloneToDialogStore((s) => s.setBringThread)
  const sinceDays = useCloneToDialogStore((s) => s.sinceDays)
  const setSinceDays = useCloneToDialogStore((s) => s.setSinceDays)
  const threads = useCloneToDialogStore((s) => s.preview?.threads)
  const counts = threads ? threadSummaryLine(threads) : null
  const people = threads?.available ? (threads.participants ?? []) : []
  return (
    <div className="space-y-1" data-testid="clone-thread-block">
      <Tick checked={bring} onChange={setBring} testId="clone-bring-thread">
        Bring the Thread{counts ? ` (${counts})` : ''}
      </Tick>
      <p className={`${muted} pl-[22px]`}>
        {people.length > 0 ? `Includes messages from: ${people.join(', ')}. ` : ''}Chatter never travels.
      </p>
      {bring && (
        <div className={`${muted} pl-[22px] flex items-center gap-1.5`}>
          <span>Only the last</span>
          {THREAD_SINCE_OPTIONS.map((o) => (
            <button
              key={o.label}
              type="button"
              data-testid={`clone-since-${o.value ?? 'all'}`}
              onClick={() => setSinceDays(o.value)}
              className={`px-1.5 py-0.5 border no-drag cursor-pointer ${
                sinceDays === o.value
                  ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                  : 'border-[var(--color-border)] text-[var(--color-text-muted)]'
              }`}
            >
              {o.label}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

const CHOICES: { value: SecretChoice; label: string }[] = [
  { value: 'copy', label: 'Copy' },
  { value: 'blank', label: 'Leave blank' },
  { value: 'skip', label: 'Skip' },
]

export function SecretsBlock(): React.JSX.Element | null {
  const preview = useCloneToDialogStore((s) => s.preview)
  const choices = useCloneToDialogStore((s) => s.secretChoices)
  const setChoice = useCloneToDialogStore((s) => s.setSecretChoice)
  if (!preview || !preview.secrets.available || preview.secrets.rows.length === 0) return null
  const callerCanCopy = preview.callerCanCopySecrets
  return (
    <div className="space-y-1.5" data-testid="clone-secrets-block">
      <p className="text-xs text-[var(--color-text-secondary)]">Secrets (names only)</p>
      {!callerCanCopy && <p className={muted}>{MEMBER_NO_COPY_LINE}</p>}
      <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
        {preview.secrets.rows.map((row) => (
          <div key={row.name} className="px-2 py-1.5 flex items-center gap-2 text-[11px]" data-testid={`clone-secret-${row.name}`}>
            <span className="font-mono text-[var(--color-text-primary)] truncate flex-1" title={row.note ?? undefined}>
              {row.name}
            </span>
            <span className="text-[var(--color-text-muted)] w-16 truncate">{row.kind}</span>
            <span className="text-[var(--color-text-muted)] w-20 truncate">{row.group === 'server' ? 'server, shared' : 'this agent'}</span>
            {CHOICES.map((c) => {
              const disabled = c.value === 'copy' && !canCopyRow(row, callerCanCopy)
              return (
                <label key={c.value} className={`flex items-center gap-1 no-drag ${disabled ? 'opacity-40' : 'cursor-pointer'}`}>
                  <input
                    type="radio"
                    name={`clone-secret-${row.name}`}
                    value={c.value}
                    data-testid={`clone-secret-${row.name}-${c.value}`}
                    checked={choices[row.name] === c.value}
                    disabled={disabled}
                    onChange={() => setChoice(row.name, c.value)}
                  />
                  <span className="text-[var(--color-text-secondary)]">{c.label}</span>
                </label>
              )
            })}
          </div>
        ))}
      </div>
      {preview.secrets.notCarried.length > 0 && (
        <p className={muted}>Not carried: {preview.secrets.notCarried.join(', ')}</p>
      )}
    </div>
  )
}

export function EnvFilesBlock({ pull }: { pull: boolean }): React.JSX.Element {
  const carrySecrets = useCloneToDialogStore((s) => s.carrySecrets)
  const setCarrySecrets = useCloneToDialogStore((s) => s.setCarrySecrets)
  const envFiles = useCloneToDialogStore((s) => s.preview?.envFiles ?? [])
  return (
    <div className="space-y-1" data-testid="clone-env-block">
      <Tick checked={carrySecrets} onChange={setCarrySecrets} testId="clone-env-files">
        Send <span className="font-mono text-[11px]">.env</span> files as files
        {envFiles.length > 0 ? ` (${envFiles.length})` : ''}
      </Tick>
      {envFiles.length > 0 && (
        <p className={`${muted} pl-[22px] font-mono break-all`}>{envFiles.join(', ')}</p>
      )}
      <p className={`${muted} pl-[22px]`}>
        Their values travel inside the bundle and need your Touch ID or vault password. Unticked, they stay behind and
        you re-add them on {pull ? 'this computer' : 'the host'}. Move them into the vault first:{' '}
        <span className="font-mono">k2 vault import .env</span>
      </p>
    </div>
  )
}

export function PasswordPromptBlock(): React.JSX.Element | null {
  const prompt = useCloneToDialogStore((s) => s.passwordPrompt)
  const answer = useCloneToDialogStore((s) => s.answerPassword)
  const ref = React.useRef<HTMLInputElement>(null)
  if (!prompt) return null
  const submit = (): void => {
    const el = ref.current
    const pw = el?.value ?? ''
    if (el) el.value = ''
    answer(pw)
  }
  return (
    <div className="border border-[var(--color-border)] px-3 py-2 space-y-1.5" data-testid="clone-password-prompt">
      <p className="text-[11px] text-[var(--color-text-secondary)]">
        {prompt.method === 'connect-password' ? 'Your login password' : 'Vault password'} to confirm: {prompt.label}
      </p>
      <div className="flex gap-2">
        <input
          ref={ref}
          type="password"
          autoFocus
          data-testid="clone-password-input"
          onKeyDown={(e) => {
            e.stopPropagation()
            if (e.key === 'Enter') submit()
            if (e.key === 'Escape') answer(null)
          }}
          className="flex-1 px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
        />
        <button type="button" onClick={submit} className="px-2 py-1 text-[11px] border border-[var(--color-border)] no-drag cursor-pointer">
          Confirm
        </button>
        <button type="button" onClick={() => answer(null)} className="px-2 py-1 text-[11px] text-[var(--color-text-muted)] no-drag cursor-pointer">
          Cancel
        </button>
      </div>
    </div>
  )
}

/** Label for the primary button: a Copy row or `.env` files need a confirm. */
export function cloneButtonLabel(choices: Record<string, SecretChoice>, carrySecrets: boolean, confirming: boolean): string {
  if (confirming) return 'Confirming…'
  return anyCopy(choices) || carrySecrets ? 'Confirm and clone' : 'Clone'
}

/** Done-screen counts from the unpack result (optional fields). */
export function doneCountsLine(r: {
  threads_imported?: number
  vault_imported?: number
  vault_skipped?: number
  cards_posted?: number
} | null): string[] {
  if (!r) return []
  const out: string[] = []
  if (typeof r.threads_imported === 'number') out.push(`Thread: imported ${r.threads_imported} message${r.threads_imported === 1 ? '' : 's'}`)
  const v: string[] = []
  if (typeof r.vault_imported === 'number') v.push(`copied ${r.vault_imported}`)
  if (typeof r.cards_posted === 'number') v.push(`left blank ${r.cards_posted} (card posted)`)
  if (typeof r.vault_skipped === 'number') v.push(`skipped ${r.vault_skipped}`)
  if (v.length > 0) out.push(`Secrets: ${v.join(' · ')}`)
  return out
}
