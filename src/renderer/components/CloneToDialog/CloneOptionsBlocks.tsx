import React from 'react'

import { Button, Input, SquareCheckbox, SquareRadio } from '@/components/ui'
import { SettingDropdown } from '@/components/Settings/controls/SettingControls'
import { targetOf, useCloneToDialogStore } from '../../stores/clone-to-dialog'
import type { CloneUnpackResult } from '../../lib/clone-to'
import type { CloneVaultOutcome } from '../../lib/clone-vault'
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

/** A ticked option: the app's SquareCheckbox with its label (the shared
 *  control Settings uses, not a one-off). */
export function Tick({ checked, onChange, children, testId, disabled }: {
  checked: boolean
  onChange: (v: boolean) => void
  children: React.ReactNode
  testId?: string
  disabled?: boolean
}): React.JSX.Element {
  return (
    <label className={`flex items-start gap-2 select-none no-drag ${disabled ? 'opacity-50' : 'cursor-pointer'}`}>
      <SquareCheckbox
        checked={checked}
        disabled={disabled}
        data-testid={testId}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-0.5 flex-shrink-0"
      />
      <span className="text-xs text-[var(--color-text-secondary)] leading-relaxed">{children}</span>
    </label>
  )
}

const muted = 'text-[11px] text-[var(--color-text-muted)] leading-relaxed'

/** The dropdown's string value for a Thread window (null = all). */
function sinceValue(days: number | null): string {
  return days === null ? 'all' : String(days)
}

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
        <div className={`${muted} pl-[22px] flex items-center gap-2`} data-testid="clone-since">
          <span>Only the last</span>
          <SettingDropdown
            ariaLabel="How much Thread history to bring"
            menuAlign="left"
            value={sinceValue(sinceDays)}
            options={THREAD_SINCE_OPTIONS.map((o) => ({ value: sinceValue(o.value), label: o.label }))}
            onChange={(v) => setSinceDays(v === 'all' ? null : Number(v))}
          />
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

export function SecretsBlock({ pull }: { pull: boolean }): React.JSX.Element | null {
  const preview = useCloneToDialogStore((s) => s.preview)
  const choices = useCloneToDialogStore((s) => s.secretChoices)
  const setChoice = useCloneToDialogStore((s) => s.setSecretChoice)
  const lookup = useCloneToDialogStore((s) => s.vaultLookup)
  const ownBoxMove = useCloneToDialogStore((s) => s.ownBoxMove)
  const setOwnBoxMove = useCloneToDialogStore((s) => s.setOwnBoxMove)
  // No destination vault (older K2, vault closed): secrets stay behind.
  if (lookup && 'note' in lookup) {
    return (
      <p className={muted} data-testid="clone-vault-note">
        {lookup.note}
      </p>
    )
  }
  if (!targetOf(lookup) || !preview || !preview.secrets.available || preview.secrets.rows.length === 0) return null
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
            <span className="text-[var(--color-text-muted)] w-20 truncate">
              {row.group === 'server' ? 'server, shared' : row.group === 'shared' ? 'shared with it' : 'this agent'}
            </span>
            {CHOICES.map((c) => {
              const disabled = c.value === 'copy' && !canCopyRow(row, callerCanCopy)
              return (
                <label key={c.value} className={`flex items-center gap-1.5 no-drag ${disabled ? 'opacity-40' : 'cursor-pointer'}`}>
                  <SquareRadio
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
      <Tick checked={ownBoxMove} onChange={setOwnBoxMove} testId="clone-own-box">
        This is my own {pull ? 'computer' : 'server'} (keeps &ldquo;given by a person&rdquo; marks)
      </Tick>
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
        <Input
          ref={ref}
          type="password"
          autoFocus
          data-testid="clone-password-input"
          onKeyDown={(e) => {
            e.stopPropagation()
            if (e.key === 'Enter') submit()
            if (e.key === 'Escape') answer(null)
          }}
          className="flex-1"
        />
        <Button variant="default" onClick={submit}>
          Confirm
        </Button>
        <Button variant="ghost" onClick={() => answer(null)}>
          Cancel
        </Button>
      </div>
    </div>
  )
}

/** Label for the primary button: a Copy row or `.env` files need a confirm. */
export function cloneButtonLabel(choices: Record<string, SecretChoice>, carrySecrets: boolean, confirming: boolean): string {
  if (confirming) return 'Confirming…'
  return anyCopy(choices) || carrySecrets ? 'Confirm and clone' : 'Clone'
}

/** Done-screen lines: the Thread import (slice B's report) and the vault
 *  outcome (slice A, names only). */
export function doneCountsLine(
  r: Pick<CloneUnpackResult, 'threads_imported' | 'threads'> | null,
  vault?: CloneVaultOutcome | null,
): string[] {
  const out: string[] = []
  if (r && typeof r.threads_imported === 'number') {
    const n = r.threads_imported
    const convs = r.threads?.conversations?.length
    const detached = r.threads?.detached ?? 0
    out.push(
      `Thread: imported ${n} message${n === 1 ? '' : 's'}` +
        (typeof convs === 'number' ? ` into ${convs} Thread${convs === 1 ? '' : 's'}` : '') +
        (detached > 0 ? ` (${detached} detached)` : '') +
        (r.threads?.pending ? '; the main one joins the clone\u2019s Chat when it first starts' : ''),
    )
  }
  if (vault) {
    if (vault.note) {
      out.push(`Secrets: ${vault.note}`)
    } else {
      const v: string[] = []
      if (vault.imported.length > 0) v.push(`copied ${vault.imported.join(', ')}`)
      if (vault.blank.length > 0) v.push(`left blank ${vault.blank.join(', ')} (card posted)`)
      if (vault.skipped.length > 0) v.push(`skipped ${vault.skipped.join(', ')}`)
      if (v.length > 0) out.push(`Secrets: ${v.join(' · ')}`)
    }
  }
  return out
}
