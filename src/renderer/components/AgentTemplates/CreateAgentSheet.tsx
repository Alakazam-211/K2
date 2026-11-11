// Create an agent (prd-agent-templates-v1 §6.1; the T5–T7 contract's T7):
// make a new agent from a template, in four steps.
//
//   1. Pick: the server's templates (`GET /cli/agent-templates`), General
//      first, then the other built-ins, then "Yours" (the library). System
//      templates (K5, the Chief of Staff) are never offered. A library card
//      names its author: its charter is someone's instructions.
//   2. Name and folder: the handle comes from the name and the folder from
//      `<hire root>/<handle>`; both can be changed. Then the template's
//      `[params]` and its first-conversation questions.
//   3. Tool: the enabled presets with their sign-in state, the template's
//      suggestion (else the server default) chosen.
//   4. Review: the daemon's own dry run (`POST /cli/agents/hire` with
//      `dryRun: true`), every row drawn. Permission rows are plain rows
//      until the permissions step (T3) adds checkboxes.
//
// Create posts the same request for real, then opens the new agent's Chat tab
// in the Thread view (the Kessel sheet's helper) and starts it with the
// template's hello, as `k2 agent hire --launch` does. The daemon decides who
// may and what's written; its refusal is shown in the sheet's words with the
// server's own underneath, and "Try again".
//
// Opened from the sidebar's add menu (`useAgentTemplatesStore`), mounted once
// in App. The window's server only. Look: the Kessel sheet's frame on the
// shared DialogFrame; K2 Design polishes it.

import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { Button, Callout, Input, Radio, Spinner, Tag, Textarea } from '@/components/ui'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { DIALOG_FIELD, DIALOG_LABEL, DIALOG_MUTED, FailedNote } from '@/components/Settings/sections/AccessDialogs'
import { SETTING_ROW_LABEL } from '@/components/Settings/controls/SettingControls'
import { primaryScope } from '@/kessel/server-scope'
import { matchAgentPreset, parseCommand, resolveAgentPreset } from '@/lib/agent-resolve'
import {
  TemplatesUnsupportedError,
  authorLabel,
  getHireRoot,
  hireErrorView,
  hireFromTemplate,
  hireStatusLabel,
  hireStepLabel,
  listTemplates,
  permissionTarget,
  pickerGroups,
  sayHello,
  showTemplate,
  type HireErrorView,
  type HireRequest,
  type HireResult,
  type TemplateDetail,
  type TemplateSummary,
} from '@/lib/agent-templates-api'
import { asArray } from '@/lib/as-array'
import { slugifyAddressToken } from '@/lib/federation'
import { openWorkspaceThread } from '@/lib/open-workspace-thread'
import { useAgentTemplatesStore } from '@/stores/agent-templates'
import { useLlmAccountsStore, watchLlmAccounts, type LlmAccountsDoc } from '@/stores/llm-accounts'
import { requestLlmSignIn } from '@/stores/llm-sign-in-request'
import { usePresetsStore, type AgentPreset } from '@/stores/presets'
import { useSettingsStore } from '@/stores/settings'
import { ReviewGroup, ReviewLine, TemplateSheetFrame } from './TemplateSheetFrame'

// ── Copy (purpose only) ──────────────────────────────────────────────

export const CREATE_AGENT_COPY = {
  title: 'Create an agent',
  pick: 'Pick what kind of agent it is',
  pickIntro: 'Each kind starts with its own charter, notes and context. You can change any of it later.',
  builtIn: 'Built into K2',
  yours: 'Yours',
  name: 'Name and folder',
  nameIntro: 'The agent works in its own folder on this server.',
  questions: 'Questions it would ask you',
  questionsHint: 'Answer any you can now. The agent asks you the rest when it starts.',
  tool: 'Choose its tool',
  toolIntro: 'The agent thinks with one of the tools on this server.',
  review: 'Review',
  reviewIntro: 'This is everything K2 will set up for the agent.',
  create: 'Create agent',
  creating: 'Creating…',
  tryAgain: 'Try again',
} as const

export const CREATE_AGENT_STEP_COUNT = 4
export const CREATE_AGENT_BUSY = 'Making the agent’s folder and setting it up…'
export const CREATE_AGENT_CLOSE_HELD = 'Can’t close while the agent is being made'
export const CREATE_AGENT_NO_TEMPLATES = 'This server has no agent templates to pick from.'
export const CREATE_AGENT_NOT_SIGNED_IN = 'This tool isn’t signed in on this server yet. The agent can start once it is.'
export const CREATE_AGENT_LIBRARY_NOTE = 'Its charter is someone’s own instructions. Read it before you create the agent.'

type Step = 'pick' | 'name' | 'tool' | 'review'
const STEP_NUMBER: Record<Step, number> = { pick: 1, name: 2, tool: 3, review: 4 }

// ── Helpers (pure; tested) ───────────────────────────────────────────

/** The handle a name gives (the daemon's slug rule), or '' when it gives none. */
export function handleFromName(name: string): string {
  return slugifyAddressToken(name) ?? ''
}

/** The default folder: `<root>/<handle>`, or null while the root is unknown. */
export function folderFor(root: string | null, handle: string): string | null {
  if (!root || !handle) return null
  return `${root.replace(/\/+$/, '')}/${handle}`
}

export function nameProblem(name: string): string | null {
  return name.trim() ? null : 'Give the agent a name.'
}

export function handleProblem(handle: string): string | null {
  if (!handle.trim()) return 'Give the agent a handle.'
  return slugifyAddressToken(handle) ? null : 'Use letters, numbers and hyphens.'
}

export function folderProblem(folder: string): string | null {
  const f = folder.trim()
  if (!f) return 'Give the agent a folder.'
  return f.startsWith('/') || f.startsWith('~') ? null : 'Give the full path to the folder.'
}

/** The tool name a preset runs (`claude`, `codex`). */
export function presetTool(p: Pick<AgentPreset, 'command'>): string {
  return parseCommand(p.command).command
}

export type SignIn = 'signed_in' | 'not_signed_in' | null

/** A preset's sign-in state on this server: null for a tool K2 doesn't sign
 *  in (or while the list loads). Signed in = a signed-in login or an API
 *  token (the `llm_sign_in` row's rule). */
export function presetSignIn(doc: LlmAccountsDoc | null | undefined, p: Pick<AgentPreset, 'command'>): SignIn {
  if (!doc) return null
  const t = doc.tools.find((x) => x.tool === presetTool(p))
  if (!t) return null
  return t.accounts.some((a) => a.state === 'signed_in' || a.kind === 'api_key') ? 'signed_in' : 'not_signed_in'
}

/** The tool chosen first: the template's suggestion when it names an enabled
 *  preset, else the server default. */
export function defaultPresetFor(
  presets: readonly AgentPreset[],
  suggested: string | null | undefined,
  serverDefault: string | null | undefined,
): string | null {
  const enabled = presets.filter((p) => !!p.enabled)
  const want = suggested && suggested !== 'default' ? matchAgentPreset(enabled, suggested) : null
  return (want ?? resolveAgentPreset(enabled, serverDefault))?.id ?? null
}

/** Only the fields with something in them. */
function filled(values: Record<string, string>): Record<string, string> | undefined {
  const out: Record<string, string> = {}
  for (const [k, v] of Object.entries(values)) if (v.trim()) out[k] = v.trim()
  return Object.keys(out).length ? out : undefined
}

/** The hire request the sheet sends. The folder goes only when the person
 *  changed it, so the daemon's own `<hire root>/<handle>` (and its "that
 *  folder is in use" check) applies otherwise. */
export function buildHireRequest(f: {
  template: string
  name: string
  handle: string
  folder: string
  defaultFolder: string | null
  preset: string | null
  params: Record<string, string>
  answers: Record<string, string>
  dryRun: boolean
}): HireRequest {
  const handle = slugifyAddressToken(f.handle) ?? f.handle.trim()
  const folder = f.folder.trim()
  const ownFolder = folder && folder !== f.defaultFolder
  return {
    template: f.template,
    name: f.name.trim(),
    handle,
    ...(ownFolder ? { path: folder } : {}),
    ...(f.preset ? { preset: f.preset } : {}),
    ...(filled(f.params) ? { params: filled(f.params) } : {}),
    ...(filled(f.answers) ? { answers: filled(f.answers) } : {}),
    dryRun: f.dryRun,
  }
}

/** The review's groups, in the plan's order. */
export function groupRows(rows: HireResult['rows']): Array<{ step: string; rows: HireResult['rows'] }> {
  const out: Array<{ step: string; rows: HireResult['rows'] }> = []
  for (const r of rows) {
    const last = out[out.length - 1]
    if (last && last.step === r.step) last.rows.push(r)
    else out.push({ step: r.step, rows: [r] })
  }
  return out
}

// ── The sheet ────────────────────────────────────────────────────────

export default function CreateAgentSheet(): React.JSX.Element | null {
  const open = useAgentTemplatesStore((s) => s.createOpen)
  if (!open) return null
  return <CreateAgentBody />
}

type Load<T> = { state: 'loading' } | { state: 'ok'; value: T } | { state: 'error'; error: unknown }

function CreateAgentBody(): React.JSX.Element {
  const close = useAgentTemplatesStore((s) => s.closeCreate)
  const scope = primaryScope()
  useEffect(() => watchLlmAccounts(scope), [scope])
  const accounts = useLlmAccountsStore((s) => s.entries[scope.id]?.doc)
  const presets = usePresetsStore((s) => s.presets)
  const presetsLoaded = usePresetsStore((s) => s.presetsLoaded)
  const serverDefault = useSettingsStore((s) => s.defaultAgent)
  useEffect(() => {
    if (!presetsLoaded) void usePresetsStore.getState().fetchPresets()
  }, [presetsLoaded])

  const [list, setList] = useState<Load<TemplateSummary[]>>({ state: 'loading' })
  const [listNonce, setListNonce] = useState(0)
  const [root, setRoot] = useState<string | null>(null)
  useEffect(() => {
    let cancelled = false
    setList({ state: 'loading' })
    listTemplates(scope).then(
      (r) => !cancelled && setList({ state: 'ok', value: asArray<TemplateSummary>(r?.templates) }),
      (error) => !cancelled && setList({ state: 'error', error }),
    )
    getHireRoot(scope).then(
      (r) => !cancelled && setRoot(r.root || null),
      () => {
        /* unknown root: the daemon still picks `<hire root>/<handle>` */
      },
    )
    return () => {
      cancelled = true
    }
  }, [scope, listNonce])

  const [step, setStep] = useState<Step>('pick')
  /** The person's pick; null: the first card. */
  const [pickedId, setPicked] = useState<string | null>(null)
  const details = useRef(new Map<string, TemplateDetail>())
  const [detail, setDetail] = useState<TemplateDetail | null>(null)
  const [detailBusy, setDetailBusy] = useState(false)
  const [detailError, setDetailError] = useState<string | null>(null)

  const [name, setName] = useState('')
  const [nameTouched, setNameTouched] = useState(false)
  /** What the person typed; null: follow the name (handle) or the handle (folder). */
  const [handleInput, setHandle] = useState<string | null>(null)
  const [folderInput, setFolder] = useState<string | null>(null)
  const [params, setParams] = useState<Record<string, string>>({})
  const [answers, setAnswers] = useState<Record<string, string>>({})
  const [showProblems, setShowProblems] = useState(false)

  /** The person's pick; null: the template's suggestion or the server default. */
  const [toolPick, setTool] = useState<string | null>(null)

  const [plan, setPlan] = useState<Load<HireResult> | null>(null)
  const [busy, setBusy] = useState(false)
  const [failed, setFailed] = useState<HireErrorView | null>(null)

  const groups = useMemo(() => (list.state === 'ok' ? pickerGroups(list.value) : { builtIn: [], yours: [] }), [list])
  const enabledPresets = useMemo(() => presets.filter((p) => !!p.enabled), [presets])
  const serverDefaultPreset = useMemo(() => resolveAgentPreset(enabledPresets, serverDefault)?.id ?? null, [enabledPresets, serverDefault])

  // The first card is chosen until the person picks.
  const picked = pickedId ?? groups.builtIn[0]?.id ?? groups.yours[0]?.id ?? null

  // The handle follows the name, and the folder the handle, until changed.
  const handle = handleInput ?? handleFromName(name)
  const defaultFolder = folderFor(root, slugifyAddressToken(handle) ?? '')
  const folderTouched = folderInput !== null
  const folder = folderInput ?? defaultFolder ?? ''

  // The template's suggestion (else the server default), until the person
  // picks (a pick that's no longer enabled falls back the same way).
  const tool =
    toolPick && enabledPresets.some((p) => p.id === toolPick)
      ? toolPick
      : defaultPresetFor(enabledPresets, detail?.agent.tool, serverDefault)

  const pickedSummary = list.state === 'ok' ? list.value.find((t) => t.id === picked) ?? null : null

  const goName = async (): Promise<void> => {
    if (!picked) return
    setDetailError(null)
    let d = details.current.get(picked) ?? null
    if (!d) {
      setDetailBusy(true)
      try {
        d = await showTemplate(scope, picked)
        details.current.set(picked, d)
      } catch (e) {
        setDetailError(hireErrorView(e).detail)
        return
      } finally {
        setDetailBusy(false)
      }
    }
    if (detail?.id !== d.id) {
      setParams({})
      setAnswers({})
      if (!nameTouched) setName(d.name)
    }
    setDetail(d)
    setShowProblems(false)
    setStep('name')
  }

  const paramProblems = useMemo(() => {
    const out: Record<string, string> = {}
    for (const [k, p] of Object.entries(detail?.params ?? {})) {
      if (p.required && !(params[k] ?? '').trim()) out[k] = 'This one is needed.'
    }
    return out
  }, [detail, params])
  const problems = {
    name: nameProblem(name),
    handle: handleProblem(handle),
    folder: folderTouched || root ? folderProblem(folder) : null,
  }
  const nameStepOk = !problems.name && !problems.handle && !problems.folder && Object.keys(paramProblems).length === 0

  const request = (dryRun: boolean): HireRequest =>
    buildHireRequest({
      template: detail?.id ?? picked ?? '',
      name,
      handle,
      folder,
      defaultFolder,
      preset: tool,
      params,
      answers,
      dryRun,
    })

  const review = async (): Promise<void> => {
    setStep('review')
    setFailed(null)
    setPlan({ state: 'loading' })
    try {
      setPlan({ state: 'ok', value: await hireFromTemplate(scope, request(true)) })
    } catch (error) {
      setPlan({ state: 'error', error })
    }
  }

  const create = async (): Promise<void> => {
    if (busy) return
    setBusy(true)
    setFailed(null)
    try {
      const made = await hireFromTemplate(scope, request(false))
      close()
      void openWorkspaceThread(made.id || made.workspaceId)
      if (made.hello) {
        sayHello(scope, made.path, made.hello).catch((e) => console.warn('[create-agent] hello not delivered:', e))
      }
    } catch (e) {
      setFailed(hireErrorView(e))
    } finally {
      setBusy(false)
    }
  }

  const signIn = (): void => {
    const p = enabledPresets.find((x) => x.id === tool)
    close()
    requestLlmSignIn(p ? { tool: presetTool(p), toolLabel: p.label } : undefined)
  }

  const ids = { name: useId(), handle: useId(), handleHint: useId(), folder: useId() }
  const paramId = useId()

  const cancelButton = (
    <Button variant="ghost" size="md" onClick={close} data-testid="create-agent-cancel">
      Cancel
    </Button>
  )
  const frame = {
    testId: 'create-agent-sheet',
    busy,
    heldReason: CREATE_AGENT_CLOSE_HELD,
    onCancel: close,
  }

  // ── Loading, refused, empty ──
  if (list.state !== 'ok' || (groups.builtIn.length === 0 && groups.yours.length === 0)) {
    const unsupported = list.state === 'error' && list.error instanceof TemplatesUnsupportedError
    return (
      <TemplateSheetFrame
        {...frame}
        title={CREATE_AGENT_COPY.title}
        focusKey={`list-${list.state}`}
        footer={
          <>
            <Button variant="ghost" size="md" onClick={close}>
              Close
            </Button>
            {list.state === 'error' && !unsupported ? (
              <Button variant="accent" size="md" onClick={() => setListNonce((n) => n + 1)} data-testid="create-agent-retry-list">
                {CREATE_AGENT_COPY.tryAgain}
              </Button>
            ) : null}
          </>
        }
      >
        {list.state === 'loading' ? (
          <div className="flex items-center gap-2" role="status" data-testid="create-agent-loading">
            <Spinner />
            <span className={DIALOG_MUTED}>Loading…</span>
          </div>
        ) : list.state === 'error' ? (
          unsupported ? (
            <p className={DIALOG_MUTED} data-testid="create-agent-unsupported">
              {(list.error as Error).message}
            </p>
          ) : (
            <FailedNote what="K2 couldn’t load the agent templates." next="Try again in a moment." detail={hireErrorView(list.error).detail} />
          )
        ) : (
          <p className={DIALOG_MUTED} data-testid="create-agent-empty">
            {CREATE_AGENT_NO_TEMPLATES}
          </p>
        )}
      </TemplateSheetFrame>
    )
  }

  // ── 1. Pick ──
  if (step === 'pick') {
    const card = (t: TemplateSummary): React.JSX.Element => (
      <label key={t.id} className="flex items-start gap-2 cursor-pointer" data-template-card={t.id}>
        <Radio
          name="create-agent-template"
          className="mt-0.5"
          value={t.id}
          checked={picked === t.id}
          onChange={() => setPicked(t.id)}
          data-testid={`create-agent-template-${t.id}`}
        />
        <span className="min-w-0">
          <span className={`${SETTING_ROW_LABEL} block`}>{t.name}</span>
          {t.summary ? <span className={`block ${DIALOG_MUTED}`}>{t.summary}</span> : null}
          {t.origin === 'library' ? (
            <span className={`block ${DIALOG_MUTED}`} data-testid={`create-agent-author-${t.id}`}>
              By {authorLabel(t.author)}
            </span>
          ) : null}
        </span>
      </label>
    )
    return (
      <TemplateSheetFrame
        {...frame}
        title={CREATE_AGENT_COPY.pick}
        intro={CREATE_AGENT_COPY.pickIntro}
        step={{ at: 1, of: CREATE_AGENT_STEP_COUNT }}
        focusKey="pick"
        onSubmit={() => void goName()}
        status={
          detailBusy ? (
            <p className="flex items-center gap-2 type-hint text-[var(--color-text-secondary)]" role="status">
              <Spinner />
              <span>Loading…</span>
            </p>
          ) : null
        }
        footer={
          <>
            {cancelButton}
            <Button type="submit" variant="accent" size="md" disabled={!picked || detailBusy} data-testid="create-agent-next">
              Next
            </Button>
          </>
        }
      >
        <fieldset className="space-y-2" data-testid="create-agent-builtins">
          <legend className={DIALOG_LABEL}>{CREATE_AGENT_COPY.builtIn}</legend>
          <div role="radiogroup" aria-label={CREATE_AGENT_COPY.builtIn} className="space-y-2">
            {groups.builtIn.map(card)}
          </div>
        </fieldset>
        {groups.yours.length ? (
          <fieldset className="space-y-2" data-testid="create-agent-yours">
            <legend className={DIALOG_LABEL}>{CREATE_AGENT_COPY.yours}</legend>
            <div role="radiogroup" aria-label={CREATE_AGENT_COPY.yours} className="space-y-2">
              {groups.yours.map(card)}
            </div>
          </fieldset>
        ) : null}
        {detailError ? (
          <FailedNote what="K2 couldn’t load that template." next="Try again, or pick another kind." detail={detailError} />
        ) : null}
      </TemplateSheetFrame>
    )
  }

  // ── 2. Name and folder ──
  if (step === 'name' && detail) {
    const shown = (p: string | null): string | null => (showProblems ? p : null)
    const paramEntries = Object.entries(detail.params ?? {})
    const questions = detail.questions ?? []
    return (
      <TemplateSheetFrame
        {...frame}
        title={CREATE_AGENT_COPY.name}
        intro={CREATE_AGENT_COPY.nameIntro}
        step={{ at: 2, of: CREATE_AGENT_STEP_COUNT }}
        focusKey="name"
        onSubmit={() => {
          if (!nameStepOk) return setShowProblems(true)
          setStep('tool')
        }}
        footer={
          <>
            <Button variant="ghost" size="md" onClick={() => setStep('pick')} data-testid="create-agent-back">
              Back
            </Button>
            <Button type="submit" variant="accent" size="md" data-testid="create-agent-next">
              Next
            </Button>
          </>
        }
      >
        <div>
          <label htmlFor={ids.name} className={DIALOG_LABEL}>
            Name
          </label>
          <Input
            id={ids.name}
            className={DIALOG_FIELD}
            value={name}
            autoComplete="off"
            aria-invalid={shown(problems.name) ? true : undefined}
            onChange={(e) => {
              setNameTouched(true)
              setName(e.target.value)
            }}
            data-testid="create-agent-name"
          />
          <Problem text={shown(problems.name)} testId="create-agent-name-problem" />
        </div>
        <div>
          <label htmlFor={ids.handle} className={DIALOG_LABEL}>
            Handle
          </label>
          <Input
            id={ids.handle}
            className={`${DIALOG_FIELD} font-mono`}
            value={handle}
            autoComplete="off"
            spellCheck={false}
            aria-invalid={shown(problems.handle) ? true : undefined}
            aria-describedby={ids.handleHint}
            onChange={(e) => {
              setHandle(e.target.value)
            }}
            data-testid="create-agent-handle"
          />
          <div id={ids.handleHint}>
            {shown(problems.handle) ? (
              <Problem text={problems.handle} testId="create-agent-handle-problem" />
            ) : (
              <p className={`${DIALOG_MUTED} mt-1`}>How you and other agents reach it.</p>
            )}
          </div>
        </div>
        <div>
          <label htmlFor={ids.folder} className={DIALOG_LABEL}>
            Folder
          </label>
          <Input
            id={ids.folder}
            className={`${DIALOG_FIELD} font-mono`}
            value={folder}
            placeholder={root ? undefined : 'Chosen by the server'}
            autoComplete="off"
            spellCheck={false}
            aria-invalid={shown(problems.folder) ? true : undefined}
            onChange={(e) => {
              setFolder(e.target.value)
            }}
            data-testid="create-agent-folder"
          />
          <Problem text={shown(problems.folder)} testId="create-agent-folder-problem" />
        </div>
        {paramEntries.map(([key, p]) => (
          <div key={key}>
            <label htmlFor={`${paramId}-${key}`} className={DIALOG_LABEL}>
              {p.label}
              {p.required ? null : <span className="text-[var(--color-text-secondary)]"> (optional)</span>}
            </label>
            <Textarea
              id={`${paramId}-${key}`}
              className={DIALOG_FIELD}
              rows={2}
              maxLength={p.max}
              value={params[key] ?? ''}
              aria-required={p.required || undefined}
              aria-invalid={shown(paramProblems[key] ?? null) ? true : undefined}
              onChange={(e) => setParams((v) => ({ ...v, [key]: e.target.value }))}
              data-testid={`create-agent-param-${key}`}
            />
            <Problem text={shown(paramProblems[key] ?? null)} testId={`create-agent-param-${key}-problem`} />
          </div>
        ))}
        {questions.length ? (
          <fieldset className="space-y-3" data-testid="create-agent-questions">
            <legend className={DIALOG_LABEL}>{CREATE_AGENT_COPY.questions}</legend>
            <p className={DIALOG_MUTED}>{CREATE_AGENT_COPY.questionsHint}</p>
            {questions.map((q) => (
              <div key={q.id}>
                <label htmlFor={`${paramId}-q-${q.id}`} className="block type-hint text-[var(--color-text-primary)] mb-1">
                  {q.ask}
                </label>
                <Textarea
                  id={`${paramId}-q-${q.id}`}
                  className={DIALOG_FIELD}
                  rows={2}
                  value={answers[q.id] ?? ''}
                  onChange={(e) => setAnswers((v) => ({ ...v, [q.id]: e.target.value }))}
                  data-testid={`create-agent-answer-${q.id}`}
                />
              </div>
            ))}
          </fieldset>
        ) : null}
      </TemplateSheetFrame>
    )
  }

  // ── 3. Tool ──
  if (step === 'tool') {
    const chosen = enabledPresets.find((p) => p.id === tool) ?? null
    const chosenSignIn = chosen ? presetSignIn(accounts, chosen) : null
    return (
      <TemplateSheetFrame
        {...frame}
        title={CREATE_AGENT_COPY.tool}
        intro={CREATE_AGENT_COPY.toolIntro}
        step={{ at: 3, of: CREATE_AGENT_STEP_COUNT }}
        focusKey="tool"
        onSubmit={() => tool && void review()}
        footer={
          <>
            <Button variant="ghost" size="md" onClick={() => setStep('name')} data-testid="create-agent-back">
              Back
            </Button>
            <Button type="submit" variant="accent" size="md" disabled={!tool} data-testid="create-agent-next">
              Next
            </Button>
          </>
        }
      >
        {!presetsLoaded ? (
          <div className="flex items-center gap-2" role="status">
            <Spinner />
            <span className={DIALOG_MUTED}>Loading…</span>
          </div>
        ) : enabledPresets.length === 0 ? (
          <p className={DIALOG_MUTED} data-testid="create-agent-no-tools">
            This server has no tools turned on. Turn one on in Settings, then come back.
          </p>
        ) : (
          <div role="radiogroup" aria-label={CREATE_AGENT_COPY.tool} className="space-y-2" data-testid="create-agent-tools">
            {enabledPresets.map((p) => {
              const s = presetSignIn(accounts, p)
              const suggested = detail?.agent.tool && detail.agent.tool !== 'default' && matchAgentPreset([p], detail.agent.tool)
              return (
                <label key={p.id} className="flex items-start gap-2 cursor-pointer" data-create-agent-tool={p.id}>
                  <Radio
                    name="create-agent-tool"
                    className="mt-0.5"
                    value={p.id}
                    checked={tool === p.id}
                    onChange={() => {
                      setTool(p.id)
                    }}
                    data-testid={`create-agent-tool-${p.id}`}
                  />
                  <span className={`${SETTING_ROW_LABEL} flex flex-wrap items-center gap-1.5`}>
                    <span aria-hidden="true" className="flex w-5 flex-shrink-0 items-center justify-center">
                      <AgentIcon agent={presetTool(p)} size={14} />
                    </span>
                    {p.label}
                    {suggested ? <Tag className="ml-1">Suggested</Tag> : null}
                    {p.id === serverDefaultPreset ? <Tag className="ml-1">Server default</Tag> : null}
                    {s === 'signed_in' ? (
                      <Tag tone="ok" className="ml-1" data-testid={`create-agent-signin-${p.id}`}>
                        Signed in
                      </Tag>
                    ) : s === 'not_signed_in' ? (
                      <Tag tone="warn" className="ml-1" data-testid={`create-agent-signin-${p.id}`}>
                        Not signed in
                      </Tag>
                    ) : null}
                  </span>
                </label>
              )
            })}
          </div>
        )}
        {chosenSignIn === 'not_signed_in' ? (
          <Callout tone="info" className="type-hint flex items-center justify-between gap-3" data-testid="create-agent-not-signed-in">
            <span>{CREATE_AGENT_NOT_SIGNED_IN}</span>
            <Button variant="quiet" className="flex-shrink-0" onClick={signIn}>
              Sign in
            </Button>
          </Callout>
        ) : null}
      </TemplateSheetFrame>
    )
  }

  // ── 4. Review ──
  const library = (plan?.state === 'ok' ? plan.value.template.origin : pickedSummary?.origin) === 'library'
  const planFailed = plan?.state === 'error' ? hireErrorView(plan.error) : null
  return (
    <TemplateSheetFrame
      {...frame}
      title={CREATE_AGENT_COPY.review}
      intro={CREATE_AGENT_COPY.reviewIntro}
      step={{ at: 4, of: CREATE_AGENT_STEP_COUNT }}
      focusKey={`review-${plan?.state ?? 'none'}`}
      onSubmit={() => (plan?.state === 'ok' ? void create() : planFailed ? void review() : undefined)}
      status={
        busy ? (
          <p className="flex items-center gap-2 type-hint text-[var(--color-text-secondary)]" role="status" data-testid="create-agent-busy">
            <Spinner />
            <span className="min-w-0">{CREATE_AGENT_BUSY}</span>
          </p>
        ) : null
      }
      footer={
        <>
          <Button
            variant="ghost"
            size="md"
            disabled={busy}
            onClick={() => setStep(planFailed?.folderProblem || failed?.folderProblem ? 'name' : 'tool')}
            data-testid="create-agent-back"
          >
            Back
          </Button>
          {planFailed ? (
            <Button type="submit" variant="accent" size="md" data-testid="create-agent-retry-review">
              {CREATE_AGENT_COPY.tryAgain}
            </Button>
          ) : (
            <Button type="submit" variant="accent" size="md" disabled={busy || plan?.state !== 'ok'} data-testid="create-agent-create">
              {busy ? CREATE_AGENT_COPY.creating : failed ? CREATE_AGENT_COPY.tryAgain : CREATE_AGENT_COPY.create}
            </Button>
          )}
        </>
      }
    >
      {plan?.state === 'loading' || !plan ? (
        <div className="flex items-center gap-2" role="status" data-testid="create-agent-review-loading">
          <Spinner />
          <span className={DIALOG_MUTED}>Checking what K2 will set up…</span>
        </div>
      ) : planFailed ? (
        <div data-testid="create-agent-review-error">
          <FailedNote what={planFailed.text} next={planFailed.folderProblem ? 'Go back to change it.' : 'Nothing was made. Try again.'} detail={planFailed.detail} />
        </div>
      ) : plan.state === 'ok' ? (
        <ReviewBody plan={plan.value} library={library} />
      ) : null}
      {failed ? (
        <div data-testid="create-agent-error">
          <FailedNote what={failed.text} next={failed.folderProblem ? 'Nothing was made. Go back to change it.' : 'Nothing was made. Fix what the server says, then try again.'} detail={failed.detail} />
        </div>
      ) : null}
    </TemplateSheetFrame>
  )
}

function Problem({ text, testId }: { text: string | null; testId: string }): React.JSX.Element | null {
  if (!text) return null
  return (
    <p className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid={testId}>
      {text}
    </p>
  )
}

/** The dry run, drawn: the agent, then every row the daemon returned (none
 *  hidden), then the charter. */
function ReviewBody({ plan, library }: { plan: HireResult; library: boolean }): React.JSX.Element {
  const open = asArray<HireResult['questions'][number]>(plan.questions).filter((q) => !q.answered)
  return (
    <>
      <div className="space-y-0.5" data-testid="create-agent-review-agent">
        <p className={SETTING_ROW_LABEL}>
          {plan.name} <span className="font-mono text-[var(--color-text-secondary)]">@{plan.handle}</span>
        </p>
        <p className={`${DIALOG_MUTED} font-mono [overflow-wrap:anywhere]`}>{plan.path}</p>
        <p className={DIALOG_MUTED}>
          {plan.template.name} template, version {plan.template.version}
          {library ? `, by ${authorLabel(plan.template.author)}` : ''}
        </p>
      </div>
      {library ? (
        <Callout tone="warn" className="type-hint" data-testid="create-agent-library-note">
          {CREATE_AGENT_LIBRARY_NOTE}
        </Callout>
      ) : null}
      <div className="space-y-3" data-testid="create-agent-rows">
        {groupRows(asArray(plan.rows)).map((g, i) => (
          <ReviewGroup key={`${g.step}-${i}`} title={hireStepLabel(g.step)} testId={`create-agent-group-${g.step}`}>
            <ul className="m-0 p-0 list-none">
              {g.rows.map((r, j) => (
                <ReviewLine
                  key={`${r.target}-${j}`}
                  what={g.step === 'permission' ? permissionTarget(r.target) : r.target}
                  state={hireStatusLabel(r.status)}
                  note={r.reason}
                  testId="create-agent-row"
                />
              ))}
            </ul>
          </ReviewGroup>
        ))}
      </div>
      {open.length ? (
        <ReviewGroup title="It will ask you" testId="create-agent-open-questions">
          <ul className="m-0 p-0 list-none">
            {open.map((q) => (
              <ReviewLine key={q.id} what={q.ask} />
            ))}
          </ul>
        </ReviewGroup>
      ) : null}
      {plan.charter ? (
        <details open={library} data-testid="create-agent-charter">
          <summary className="type-row text-[var(--color-text-primary)] cursor-pointer">Charter</summary>
          <pre className="mt-1 whitespace-pre-wrap type-hint text-[var(--color-text-secondary)] font-mono select-text">{plan.charter}</pre>
        </details>
      ) : null}
    </>
  )
}
