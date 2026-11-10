// K2's own form for this server's GitHub credential (0.46.10,
// `compute.git-access-setup`). It opens from the permission card's "Set up
// GitHub access" and posts straight to `POST /cli/compute/git-access/setup`
// as this person: the route `k2 compute git-access setup` uses, so the key
// or token goes into this server's vault there. It never goes through the
// card, the Thread or the agent; the card's Allow follows only once the
// route said yes.
//
// The secret-card rule for the values (SecretCard.tsx): the key and the
// token are uncontrolled fields, read from the DOM once, on Save, and never
// put in React state, a store or a log. A chosen .pem file is read straight
// into the key field. Both fields are emptied after a successful save. The
// App ID isn't secret, but it is read the same way.

import { useRef, useState, type JSX } from 'react'
import { daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import { Radio } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'

/** The setup route, relative to `/cli/`. */
export const GIT_ACCESS_SETUP_ROUTE = 'compute/git-access/setup'

/** What the route answers (never the key or the token). */
export interface GitAccessCredential {
  kind: 'app' | 'token'
  appId?: string
  app?: string
}

export type GitAccessKind = 'app' | 'token'

/** The same guidance as `k2 compute git-access --help`. */
export const GIT_ACCESS_GUIDANCE =
  'Give it Repository permissions: Contents read-only, on just the repos you will grant (install the App on them, or pick them for the token).'

const fieldCls =
  'w-full px-2 py-1 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] no-drag'
const buttonCls =
  'px-2 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer disabled:opacity-50 no-drag'
const mutedCls = 'text-[10px] text-[var(--color-text-muted)]'

/** Why these values can't be sent yet, or null (the daemon checks again). */
export function gitAccessProblem(kind: GitAccessKind, appId: string, key: string, token: string): string | null {
  if (kind === 'app') {
    if (!/^\d+$/.test(appId.trim())) return "The App ID is the number on the App's settings page."
    if (!key.includes('PRIVATE KEY')) return 'Paste the private key (.pem), or choose the file.'
    return null
  }
  if (!token.trim()) return 'Paste the fine-grained token (github_pat_…).'
  return null
}

export function GitAccessSetupForm({
  scope,
  onDone,
  onCancel,
}: {
  scope: ServerScope
  onDone: (c: GitAccessCredential) => void
  onCancel: () => void
}): JSX.Element {
  const [kind, setKind] = useState<GitAccessKind>('app')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const appIdRef = useRef<HTMLInputElement>(null)
  const keyRef = useRef<HTMLTextAreaElement>(null)
  const tokenRef = useRef<HTMLInputElement>(null)

  const readFile = async (file: File | undefined): Promise<void> => {
    if (!file || !keyRef.current) return
    try {
      keyRef.current.value = await file.text()
      setError(null)
    } catch {
      setError("K2 couldn't read that file. Paste the key instead.")
    }
  }

  const submit = async (): Promise<void> => {
    const appId = appIdRef.current?.value ?? ''
    const key = keyRef.current?.value ?? ''
    const token = tokenRef.current?.value ?? ''
    const problem = gitAccessProblem(kind, appId, key, token)
    if (problem) {
      setError(problem)
      return
    }
    setBusy(true)
    setError(null)
    try {
      const body = kind === 'app' ? { app_id: appId.trim(), key_pem: key } : { token: token.trim() }
      const r = await daemonCliPost<{ credential: GitAccessCredential }>(scope, GIT_ACCESS_SETUP_ROUTE, body)
      if (keyRef.current) keyRef.current.value = ''
      if (tokenRef.current) tokenRef.current.value = ''
      onDone(r.credential)
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <form
      data-testid="git-access-form"
      className="flex flex-col gap-1.5"
      onSubmit={(e) => {
        e.preventDefault()
        void submit()
      }}
    >
      <div className="text-[11px] text-[var(--color-text-primary)]">Set up GitHub access for compute jobs</div>
      <div data-testid="git-access-guidance" className={mutedCls}>
        {GIT_ACCESS_GUIDANCE}
      </div>
      <div role="radiogroup" aria-label="Credential" className="flex flex-col gap-1">
        <label className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-primary)]">
          <Radio data-testid="git-access-kind-app" name="git-access-kind" checked={kind === 'app'} onChange={() => setKind('app')} />
          A GitHub App (preferred: 1-hour tokens, one repo each)
        </label>
        <label className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-primary)]">
          <Radio data-testid="git-access-kind-token" name="git-access-kind" checked={kind === 'token'} onChange={() => setKind('token')} />
          A fine-grained token, read-only
        </label>
      </div>
      {kind === 'app' ? (
        <>
          <input
            ref={appIdRef}
            data-testid="git-access-app-id"
            aria-label="App ID"
            inputMode="numeric"
            autoComplete="off"
            placeholder="App ID (the App's settings page)"
            className={fieldCls}
          />
          <textarea
            ref={keyRef}
            data-testid="git-access-key"
            aria-label="Private key"
            autoComplete="off"
            spellCheck={false}
            rows={4}
            placeholder="-----BEGIN PRIVATE KEY----- (paste the .pem)"
            className={`${fieldCls} font-mono rounded-[var(--radius-field-multiline)]`}
          />
          <label className={`${mutedCls} flex items-center gap-1.5`}>
            Or choose the .pem file:
            <input
              data-testid="git-access-key-file"
              type="file"
              accept=".pem,application/x-pem-file"
              className="text-[10px] no-drag"
              onChange={(e) => {
                void readFile(e.target.files?.[0])
                e.target.value = ''
              }}
            />
          </label>
          <div className={mutedCls}>Delete the .pem afterwards: the key lives in this server's vault.</div>
        </>
      ) : (
        <>
          <input
            ref={tokenRef}
            data-testid="git-access-token"
            aria-label="Fine-grained token"
            type="password"
            autoComplete="new-password"
            placeholder="github_pat_…"
            className={fieldCls}
          />
          <div className={mutedCls}>
            Nodes get this token itself (GitHub can&apos;t mint short-lived ones from it): give it an expiry. Classic tokens and your
            gh login are refused.
          </div>
        </>
      )}
      {error ? (
        <div data-testid="git-access-error" className="text-[10px] text-[var(--color-status-error)]">
          {error}
        </div>
      ) : null}
      <div className={mutedCls}>It goes straight into this server&apos;s vault. The agent never sees it.</div>
      <div className="flex gap-1.5">
        <button type="submit" data-testid="git-access-save" disabled={busy} className={buttonCls}>
          Save
        </button>
        <button type="button" data-testid="git-access-cancel" disabled={busy} onClick={onCancel} className={buttonCls}>
          Cancel
        </button>
      </div>
    </form>
  )
}
