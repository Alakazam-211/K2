// The K5 sheet (prd-setup-checklist-v1 §10.1, `k5-first-run-contract.md`):
// make K5, K2's own agent on this server, in two steps.
//
//   1. Name and folder: from the daemon's `k5Defaults`, all editable. The
//      folder is `<home>/k2-agents/<handle>`, so the handle names it.
//   2. What K5 thinks with: the claude, codex and grok tools signed in on
//      this server, with the server default chosen.
//
// Create K5 posts `first-run/k5`, then opens K5's Chat tab in the Thread
// view (asked for explicitly, not this client's "New agents open in"). The
// daemon decides who may (owner token, Owner, Admin) and refuses anything
// else; its refusal is shown as it says it. An Admin creator is told K5 asks
// an Owner before changing the organization (the daemon leaves
// `organization.manage` out of an Admin's grant).
//
// Opened by `useK5SheetStore` (the offer after a sign-in, the checklist's
// `k5_sheet`), mounted once in App. The window's server only.
//
// Look: `ui/` primitives and tokens only, so every Style draws it; K2 Design
// polishes it before the `k5FirstRun` switch goes on.

import { useEffect, useMemo, useState } from 'react'
import { Button, Callout, DialogFrame, DialogScrim, Input, Radio, cx } from '@/components/ui'
import { daemonCliGet } from '@/lib/daemon-cli'
import { openWorkspaceThread } from '@/lib/open-workspace-thread'
import { primaryScope } from '@/kessel/server-scope'
import { resolveAgentCommand } from '@/lib/agent-resolve'
import { useFirstRunStore, watchFirstRun, type K5Defaults } from '@/stores/first-run'
import { useK5SheetStore } from '@/stores/k5-sheet'
import { useLlmAccountsStore, watchLlmAccounts, type LlmAccountsDoc } from '@/stores/llm-accounts'
import { usePresetsStore } from '@/stores/presets'
import { useSettingsStore } from '@/stores/settings'

/** The contract's copy (purpose only). */
export const K5_SHEET_COPY = {
  step1: 'Name and folder',
  step2: 'What K5 thinks with',
  adminCreator: 'K5 will ask an Owner before changing your organization.',
  create: 'Create K5',
} as const

/** The tools K5 can think with, in the checklist's order. */
export const K5_TOOLS = ['claude', 'codex', 'grok'] as const

export const K5_NO_TOOL_TEXT = 'Sign in to Claude, Codex or Grok first, then come back here.'
export const K5_EXISTS_TEXT = 'K5 is already on this server.'
export const K5_OFF_TEXT = "K5 isn't turned on for this server."

export interface K5Tool {
  tool: string
  display: string
}

/** The claude, codex and grok tools with a signed-in subscription or an
 *  API token (the `llm_sign_in` row's rule). */
export function signedInK5Tools(doc: LlmAccountsDoc | null | undefined): K5Tool[] {
  if (!doc) return []
  const out: K5Tool[] = []
  for (const name of K5_TOOLS) {
    const t = doc.tools.find((x) => x.tool === name)
    if (!t) continue
    if (t.accounts.some((a) => a.state === 'signed_in' || a.kind === 'api_key')) {
      out.push({ tool: t.tool, display: t.display || t.tool })
    }
  }
  return out
}

/** The server default when it's one of `tools`, else the first of them. */
export function defaultK5Tool(tools: readonly K5Tool[], serverDefault: string | null | undefined): string | null {
  if (serverDefault && tools.some((t) => t.tool === serverDefault)) return serverDefault
  return tools[0]?.tool ?? null
}

/** The folder K5 gets for `handle`: the default folder's parent plus the
 *  handle (the daemon's `<home>/k2-agents/<handle>`). */
export function k5FolderFor(defaults: K5Defaults | null, handle: string): string {
  const h = handle.trim()
  const path = defaults?.path ?? ''
  const cut = path.lastIndexOf('/')
  const parent = cut > 0 ? path.slice(0, cut) : '~/k2-agents'
  return h ? `${parent}/${h}` : `${parent}/`
}

/** Admin rather than Owner or the owner token, from `auth/whoami`. */
export function isAdminCreator(who: unknown): boolean {
  if (!who || typeof who !== 'object') return false
  const w = who as { owner?: unknown; role?: unknown }
  return w.owner !== true && w.role === 'admin'
}

/** A handle the daemon can name a folder with: no slashes, not `.`/`..`. */
export function handleProblem(handle: string): string | null {
  const h = handle.trim()
  if (!h) return 'Give K5 a folder name.'
  if (/[/\\]/.test(h) || h === '.' || h === '..') return "The folder name can't have slashes."
  return null
}

function errorMessage(e: unknown): string {
  const code = (e as { code?: unknown } | null)?.code
  if (code === 'k5_exists') return K5_EXISTS_TEXT
  if (code === 'k5_first_run_off') return K5_OFF_TEXT
  return (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '')
}

/** The server's default agent as a tool name (`claude`), from Settings'
 *  `defaultAgent` and the presets. */
function useServerDefaultTool(): string | null {
  const presets = usePresetsStore((s) => s.presets)
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  return useMemo(() => resolveAgentCommand(presets, defaultAgent)?.command ?? defaultAgent ?? null, [presets, defaultAgent])
}

export default function K5Sheet(): React.JSX.Element | null {
  const isOpen = useK5SheetStore((s) => s.isOpen)
  if (!isOpen) return null
  return <K5SheetBody />
}

function K5SheetBody(): React.JSX.Element {
  const close = useK5SheetStore((s) => s.close)
  const scope = primaryScope()
  useEffect(() => watchFirstRun(scope), [scope])
  useEffect(() => watchLlmAccounts(scope), [scope])

  const firstRun = useFirstRunStore((s) => s.entries[scope.id])
  const doc = firstRun?.doc ?? null
  const defaults = doc?.k5Defaults ?? null
  const seat = doc?.k5 ?? null
  const accounts = useLlmAccountsStore((s) => s.entries[scope.id]?.doc)
  const tools = useMemo(() => signedInK5Tools(accounts), [accounts])
  const serverDefault = useServerDefaultTool()

  const [step, setStep] = useState<1 | 2>(1)
  const [name, setName] = useState('')
  const [handle, setHandle] = useState('')
  const [touched, setTouched] = useState(false)
  const [tool, setTool] = useState<string | null>(null)
  const [admin, setAdmin] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Fill step 1 from the daemon's defaults until the person types.
  useEffect(() => {
    if (!defaults || touched) return
    setName(defaults.name)
    setHandle(defaults.handle)
  }, [defaults, touched])

  // The server default, until the person picks (or it stops being signed in).
  useEffect(() => {
    if (tool && tools.some((t) => t.tool === tool)) return
    setTool(defaultK5Tool(tools, serverDefault))
  }, [tools, serverDefault, tool])

  // Who is making K5: the daemon's own answer, as other surfaces read it.
  useEffect(() => {
    let cancelled = false
    daemonCliGet<unknown>(scope, 'auth/whoami')
      .then((who) => {
        if (!cancelled) setAdmin(isAdminCreator(who))
      })
      .catch(() => {
        /* unknown: no line (the daemon still decides what K5 is granted) */
      })
    return () => {
      cancelled = true
    }
  }, [scope])

  const problem = handleProblem(handle)
  const loading = !doc && !firstRun?.error

  const create = async (): Promise<void> => {
    if (!tool || problem || busy) return
    setBusy(true)
    setError(null)
    try {
      const made = await useFirstRunStore.getState().createK5({ tool, name: name.trim(), handle: handle.trim() }, scope)
      close()
      void openWorkspaceThread(made.workspaceId)
    } catch (e) {
      setError(errorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  const openK5 = (): void => {
    if (!seat) return
    close()
    void openWorkspaceThread(seat.workspaceId)
  }

  const title = step === 1 ? K5_SHEET_COPY.step1 : K5_SHEET_COPY.step2

  return (
    <>
      <DialogScrim />
      <DialogFrame
        onCancel={close}
        cancelHeld={busy}
        role="dialog"
        aria-modal="true"
        aria-label={K5_SHEET_COPY.create}
        data-testid="k5-sheet"
        className="w-[min(420px,calc(100vw-2rem))] p-5 flex flex-col gap-4"
      >
        <div className="pr-6">
          <div className="type-hint text-[var(--color-text-muted)]" data-testid="k5-step">
            Step {step} of 2
          </div>
          <h2 className="m-0 type-heading text-[var(--color-text-primary)]">{title}</h2>
        </div>

        {seat ? (
          <div className="flex flex-col gap-3" data-testid="k5-exists">
            <p className="m-0 type-row text-[var(--color-text-secondary)]">{K5_EXISTS_TEXT}</p>
            <div className="flex justify-end gap-2">
              <Button variant="accent" size="md" onClick={openK5} data-testid="k5-open">
                Open {seat.name}
              </Button>
            </div>
          </div>
        ) : loading ? (
          <p className="m-0 type-hint text-[var(--color-text-muted)]" data-testid="k5-loading">
            Loading…
          </p>
        ) : !defaults ? (
          <p className="m-0 type-row text-[var(--color-text-secondary)]" data-testid="k5-off">
            {firstRun?.error ?? K5_OFF_TEXT}
          </p>
        ) : step === 1 ? (
          <div className="flex flex-col gap-3">
            <label className="flex flex-col gap-1">
              <span className="type-hint text-[var(--color-text-muted)]">Name</span>
              <Input
                autoFocus
                value={name}
                onChange={(e) => {
                  setTouched(true)
                  setName(e.target.value)
                }}
                data-testid="k5-name"
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className="type-hint text-[var(--color-text-muted)]">Folder name</span>
              <Input
                value={handle}
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => {
                  setTouched(true)
                  setHandle(e.target.value)
                }}
                data-testid="k5-handle"
              />
              <span className="type-hint font-mono text-[var(--color-text-muted)] break-all" data-testid="k5-folder">
                {k5FolderFor(defaults, handle)}
              </span>
            </label>
            {problem && touched && (
              <p className="m-0 type-hint text-[var(--color-status-error-text)]" data-testid="k5-handle-problem">
                {problem}
              </p>
            )}
            <div className="flex justify-end gap-2">
              <Button variant="ghost" size="md" onClick={close}>
                Cancel
              </Button>
              <Button
                variant="accent"
                size="md"
                disabled={!!problem || !name.trim()}
                onClick={() => setStep(2)}
                data-testid="k5-next"
              >
                Next
              </Button>
            </div>
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            {tools.length === 0 ? (
              <p className="m-0 type-row text-[var(--color-text-secondary)]" data-testid="k5-no-tool">
                {K5_NO_TOOL_TEXT}
              </p>
            ) : (
              <div role="radiogroup" aria-label={K5_SHEET_COPY.step2} className="flex flex-col gap-1" data-testid="k5-tools">
                {tools.map((t) => (
                  <label
                    key={t.tool}
                    className={cx(
                      'flex items-center gap-2 px-2 py-1.5 type-row cursor-pointer',
                      tool === t.tool ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]',
                    )}
                  >
                    <Radio
                      name="k5-tool"
                      value={t.tool}
                      checked={tool === t.tool}
                      onChange={() => setTool(t.tool)}
                      data-testid={`k5-tool-${t.tool}`}
                    />
                    <span className="flex-1">{t.display}</span>
                    {t.tool === serverDefault && <span className="type-hint text-[var(--color-text-muted)]">Server default</span>}
                  </label>
                ))}
              </div>
            )}
            {admin && (
              <p className="m-0 type-hint text-[var(--color-text-secondary)]" data-testid="k5-admin-line">
                {K5_SHEET_COPY.adminCreator}
              </p>
            )}
            {error && (
              <Callout tone="error" role="alert" data-testid="k5-error">
                {error}
              </Callout>
            )}
            <div className="flex justify-end gap-2">
              <Button variant="ghost" size="md" disabled={busy} onClick={() => setStep(1)} data-testid="k5-back">
                Back
              </Button>
              <Button
                variant="accent"
                size="md"
                disabled={busy || !tool || !!problem}
                onClick={() => void create()}
                data-testid="k5-create"
              >
                {busy ? 'Creating…' : K5_SHEET_COPY.create}
              </Button>
            </div>
          </div>
        )}
      </DialogFrame>
    </>
  )
}
