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
//      `dryRun: true`), every row drawn. Each permission the hire would turn
//      on, or ask for on a card, is a checkbox the person can untick (sent
//      as `declinePermissions`); one already on, or one that stays off, is
//      a disabled box with the reason (T3).
//
// Create posts the same request for real, then opens the new agent's Chat tab
// in the Thread view (the Kessel sheet's helper). The daemon starts the agent
// and delivers the template's hello itself (`helloDelivered: true`); only an
// older daemon, which leaves it out, gets the hello sent from here, as
// `k2 agent hire --launch` does. The daemon decides who
// may and what's written; its refusal is shown in the sheet's words with the
// server's own underneath, and "Try again".
//
// Opened from the sidebar's add menu (`useAgentTemplatesStore`), mounted once
// in App. The window's server only.
//
// Look: the Kessel sheet's frame on the shared DialogFrame, one height for
// every step (TemplateSheetFrame). Each kind is a card you pick (ui/RadioTile:
// its icon, name and line, the picked one edged in the accent), grouped under
// "Built into K2" and "Yours"; the tools are the Kessel sheet's list; the
// Review is the agent on one card, each row with an Edit back to its step,
// then what K2 sets up on a second, then the charter, folded, as formatted
// text on an inset well.

import { useEffect, useId, useMemo, useRef, useState } from 'react'
import remarkGfm from 'remark-gfm'
import { Button, Callout, Checkbox, Input, Radio, RadioTile, Spinner, Surface, Tag, Textarea, TileIcon } from '@/components/ui'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import Markdown from '@/components/Markdown/Markdown'
import { FEATURE_ICONS } from '@/components/icons/feature-icons'
import {
  AgentsIcon,
  BookmarkIcon,
  ChecklistIcon,
  ChevronRightIcon,
  GlobeIcon,
  HeadphonesIcon,
  OrgChartIcon,
  PaletteIcon,
  PencilIcon,
  ServerIcon,
  WikiIcon,
  type K2IconProps,
} from '@/components/icons/K2Icons'
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
  listTemplates,
  permissionChoosable,
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
import { ReviewCard, ReviewItems, ReviewRow, ReviewSection, TemplateSheetFrame } from './TemplateSheetFrame'
import { hireRowWords, rowNote } from './review-words'
import { PRODUCT_NAME } from '@/lib/brand'

// ── Copy (purpose only) ──────────────────────────────────────────────

export const CREATE_AGENT_COPY = {
  title: 'Create an agent',
  pick: 'Pick what kind of agent it is',
  pickIntro: 'Each kind starts with its own charter, notes and context. You can change any of it later.',
  builtIn: `Built into ${PRODUCT_NAME}`,
  yours: 'Yours',
  name: 'Name and folder',
  nameIntro: 'The agent works in its own folder on this server.',
  questions: 'Questions it would ask you',
  questionsHint: 'Answer any you can now. The agent asks you the rest when it starts.',
  tool: 'Choose its tool',
  toolIntro: 'The agent thinks with one of the tools on this server.',
  review: 'Review',
  reviewIntro: `This is everything ${PRODUCT_NAME} will set up for the agent.`,
  create: 'Create agent',
  creating: 'Creating…',
  tryAgain: 'Try again',
} as const

export const CREATE_AGENT_STEP_COUNT = 4
export const CREATE_AGENT_BUSY = 'Making the agent’s folder and setting it up…'
export const CREATE_AGENT_CLOSE_HELD = 'Can’t close while the agent is being made'
export const CREATE_AGENT_NO_TEMPLATES = 'This server has no agent templates to pick from.'
export const CREATE_AGENT_NOT_SIGNED_IN = 'This tool isn’t signed in on this server yet. The agent can start once it is.'
/** No tool K2 signs in to is signed in on this server. */
export const CREATE_AGENT_NO_TOOL = 'No tool is signed in on this server yet. The agent can start once one is.'
export const CREATE_AGENT_NO_TOOLS_ON = 'This server has no tools turned on. Turn one on in Settings, then come back.'
export const CREATE_AGENT_LIBRARY_NOTE = 'Its charter is someone’s own instructions. Read it before you create the agent.'

type Step = 'pick' | 'name' | 'tool' | 'review'

/** Each built-in kind's icon (K2's own set); a saved template gets the
 *  bookmark, and a kind K2 doesn't know the agent. */
const TEMPLATE_ICONS: Record<string, (p: K2IconProps) => React.JSX.Element> = {
  general: AgentsIcon,
  developer: FEATURE_ICONS.terminal,
  reviewer: ChecklistIcon,
  writer: PencilIcon,
  lead: OrgChartIcon,
  support: HeadphonesIcon,
  'app-owner': GlobeIcon,
  designer: PaletteIcon,
  ops: ServerIcon,
  researcher: WikiIcon,
}

export function templateIconFor(t: Pick<TemplateSummary, 'id' | 'origin'>): (p: K2IconProps) => React.JSX.Element {
  if (t.origin === 'library') return BookmarkIcon
  return TEMPLATE_ICONS[t.id] ?? AgentsIcon
}

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

/** A required template field left empty: names it, since the agent can't
 *  be made without it (unlike the questions, which it asks later). */
export function paramProblem(label: string): string {
  return `Answer “${label.trim()}” to go on. The agent needs it from the start.`
}

/** A name that gives no handle (only symbols, say) while the handle still
 *  follows it. */
export const NAME_GIVES_NO_HANDLE = 'Use at least one letter or number in the name.'

/** The name step's problems. While the handle follows the name (and the
 *  folder the handle), only the name's problem shows, covering what it
 *  gives them; the handle's and folder's own show once each is edited. */
export function nameStepProblems(f: {
  name: string
  handle: string
  folder: string
  handleEdited: boolean
  folderEdited: boolean
}): { name: string | null; handle: string | null; folder: string | null } {
  const own = nameProblem(f.name)
  const handle = handleProblem(f.handle)
  return {
    name: own ?? (!f.handleEdited && handle ? NAME_GIVES_NO_HANDLE : null),
    handle: f.handleEdited ? handle : null,
    // Unedited, the folder is `<root>/<handle>` (or the server's pick), so
    // its only problem is the handle's.
    folder: f.folderEdited ? folderProblem(f.folder) : null,
  }
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
  /** Suggested permissions the person unticked. */
  declined?: string[]
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
    ...(f.declined?.length ? { declinePermissions: f.declined } : {}),
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
  // The template to start on (Save as template's "Create an agent from it").
  const [pickedId, setPicked] = useState<string | null>(() => useAgentTemplatesStore.getState().createStart)
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
  // Permissions unticked on the Review (ids, as the plan's rows name them).
  const [declined, setDeclined] = useState<string[]>([])
  const [busy, setBusy] = useState(false)
  const [failed, setFailed] = useState<HireErrorView | null>(null)

  const groups = useMemo(() => (list.state === 'ok' ? pickerGroups(list.value) : { builtIn: [], yours: [] }), [list])
  const enabledPresets = useMemo(() => presets.filter((p) => !!p.enabled), [presets])
  const serverDefaultPreset = useMemo(() => resolveAgentPreset(enabledPresets, serverDefault)?.id ?? null, [enabledPresets, serverDefault])

  // The first card is chosen until the person picks.
  const offered = (id: string | null): boolean => !!id && [...groups.builtIn, ...groups.yours].some((t) => t.id === id)
  const picked = (offered(pickedId) ? pickedId : null) ?? groups.builtIn[0]?.id ?? groups.yours[0]?.id ?? null
  // A starting template may sit under "Yours", past the fold: bring it into view.
  const startShown = useRef(false)
  useEffect(() => {
    if (startShown.current || list.state !== 'ok' || !pickedId || !offered(pickedId)) return
    startShown.current = true
    const card = Array.from(document.querySelectorAll<HTMLElement>('[data-template-card]')).find((el) => el.dataset.templateCard === pickedId)
    card?.scrollIntoView?.({ block: 'nearest' })
  })

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
      if (p.required && !(params[k] ?? '').trim()) out[k] = paramProblem(p.label)
    }
    return out
  }, [detail, params])
  const problems = nameStepProblems({
    name,
    handle,
    folder,
    handleEdited: handleInput !== null,
    folderEdited: folderTouched,
  })
  const nameStepOk = !problems.name && !problems.handle && !problems.folder && Object.keys(paramProblems).length === 0

  // Only what this plan offers to untick: a template changed since keeps no
  // stale id (the daemon refuses one it doesn't suggest).
  const declinedNow = (): string[] =>
    plan?.state === 'ok'
      ? declined.filter((id) => asArray<HireResult['rows'][number]>(plan.value.rows).some((r) => r.step === 'permission' && r.target === id && permissionChoosable(r.status)))
      : []

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
      declined: dryRun ? [] : declinedNow(),
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
      const made = await hireFromTemplate(scope, { ...request(false), launch: true })
      close()
      void openWorkspaceThread(made.id || made.workspaceId)
      // Fallback for an older daemon only: a newer one has already said hello.
      if (made.hello && made.helloDelivered !== true) {
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
  const pickId = useId()

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
            <FailedNote what={`${PRODUCT_NAME} couldn’t load the agent templates.`} next="Try again in a moment." detail={hireErrorView(list.error).detail} />
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
    const card = (t: TemplateSummary): React.JSX.Element => {
      const Icon = templateIconFor(t)
      return (
        <RadioTile
          key={t.id}
          name="create-agent-template"
          value={t.id}
          checked={picked === t.id}
          onChange={() => setPicked(t.id)}
          aria-describedby={`${pickId}-${t.id}`}
          leading={
            <TileIcon>
              <Icon size={16} />
            </TileIcon>
          }
          tileProps={{ 'data-template-card': t.id }}
          data-testid={`create-agent-template-${t.id}`}
        >
          <span className={`${SETTING_ROW_LABEL} block`}>{t.name}</span>
          <span id={`${pickId}-${t.id}`} className="block">
            {t.summary ? <span className={`block ${DIALOG_MUTED}`}>{t.summary}</span> : null}
            {t.origin === 'library' ? (
              <span className="block type-hint text-[var(--color-text-muted)] mt-0.5" data-testid={`create-agent-author-${t.id}`}>
                By {authorLabel(t.author)}
              </span>
            ) : null}
          </span>
        </RadioTile>
      )
    }
    const group = (title: string, list: TemplateSummary[], testId: string): React.JSX.Element => (
      // No margin class here: the group's space-y sets the gap between headings.
      <fieldset className="p-0 border-0 min-w-0" data-testid={testId}>
        <legend className="p-0 mb-2 type-hint font-medium text-[var(--color-text-secondary)]">{title}</legend>
        <div className="space-y-2">{list.map(card)}</div>
      </fieldset>
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
        <div role="radiogroup" aria-label={CREATE_AGENT_COPY.pick} className="space-y-5">
          {groups.builtIn.length ? group(CREATE_AGENT_COPY.builtIn, groups.builtIn, 'create-agent-builtins') : null}
          {groups.yours.length ? group(CREATE_AGENT_COPY.yours, groups.yours, 'create-agent-yours') : null}
        </div>
        {detailError ? (
          <FailedNote what={`${PRODUCT_NAME} couldn’t load that template.`} next="Try again, or pick another kind." detail={detailError} />
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
            className={DIALOG_FIELD}
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
            className={DIALOG_FIELD}
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
    // The Kessel sheet's tool list: each tool's icon and name, the template's
    // suggestion and the server default tagged, and a tool that isn't signed
    // in said so quietly. Signed in is the usual case, so it isn't tagged.
    const states = enabledPresets.map((p) => presetSignIn(accounts, p))
    const noneSignedIn = states.some((x) => x !== null) && !states.includes('signed_in')
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
            {CREATE_AGENT_NO_TOOLS_ON}
          </p>
        ) : (
          <div role="radiogroup" aria-label={CREATE_AGENT_COPY.tool} className="space-y-2" data-testid="create-agent-tools">
            {enabledPresets.map((p, i) => {
              const signIn = states[i]
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
                    {signIn === 'not_signed_in' ? (
                      <span className="ml-1 type-hint text-[var(--color-text-secondary)]" data-testid={`create-agent-signin-${p.id}`}>
                        Not signed in
                      </span>
                    ) : null}
                  </span>
                </label>
              )
            })}
          </div>
        )}
        {chosenSignIn === 'not_signed_in' ? (
          <Callout tone="info" className="type-hint flex items-center justify-between gap-3" data-testid="create-agent-not-signed-in">
            <span>{noneSignedIn ? CREATE_AGENT_NO_TOOL : CREATE_AGENT_NOT_SIGNED_IN}</span>
            <Button variant="quiet" className="flex-shrink-0" onClick={signIn} data-testid="create-agent-sign-in">
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
  const fixInName = !!(planFailed?.folderProblem || failed?.folderProblem)
  const chosenPreset = enabledPresets.find((p) => p.id === tool) ?? null
  const chosenTool = chosenPreset ? { label: chosenPreset.label, tool: presetTool(chosenPreset), signIn: presetSignIn(accounts, chosenPreset) } : null
  return (
    <TemplateSheetFrame
      {...frame}
      title={CREATE_AGENT_COPY.review}
      intro={CREATE_AGENT_COPY.reviewIntro}
      step={{ at: 4, of: CREATE_AGENT_STEP_COUNT }}
      focusKey={`review-${plan?.state ?? 'none'}`}
      onSubmit={() => (fixInName ? setStep('name') : plan?.state === 'ok' ? void create() : planFailed ? void review() : undefined)}
      status={
        busy ? (
          <p className="flex items-center gap-2 type-hint text-[var(--color-text-secondary)]" role="status" data-testid="create-agent-busy">
            <Spinner />
            <span className="min-w-0">{CREATE_AGENT_BUSY}</span>
          </p>
        ) : null
      }
      footer={
        fixInName ? (
          // The fix is the name or folder: Back is the way on, and the only
          // main button (the line above says to go back).
          <Button variant="accent" size="md" onClick={() => setStep('name')} data-testid="create-agent-back">
            Back
          </Button>
        ) : (
        <>
          <Button variant="ghost" size="md" disabled={busy} onClick={() => setStep('tool')} data-testid="create-agent-back">
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
        )
      }
    >
      {plan?.state === 'loading' || !plan ? (
        <div className="flex items-center gap-2" role="status" data-testid="create-agent-review-loading">
          <Spinner />
          <span className={DIALOG_MUTED}>Checking what {PRODUCT_NAME} will set up…</span>
        </div>
      ) : planFailed ? (
        <div data-testid="create-agent-review-error">
          <FailedNote what={planFailed.text} next={planFailed.folderProblem ? 'Nothing was made. Go back to change it.' : 'Nothing was made. Try again.'} detail={planFailed.detail} />
        </div>
      ) : plan.state === 'ok' ? (
        <ReviewBody
          plan={plan.value}
          library={library}
          tool={chosenTool}
          busy={busy}
          declined={declined}
          onToggle={(id) => setDeclined((d) => (d.includes(id) ? d.filter((x) => x !== id) : [...d, id]))}
          onEdit={(to) => {
            setFailed(null)
            setStep(to)
          }}
        />
      ) : null}
      {failed ? (
        <div data-testid="create-agent-error">
          <FailedNote what={failed.text} next={failed.folderProblem ? 'Nothing was made. Go back to change it.' : 'Nothing was made. Try again.'} detail={failed.detail} />
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

/** The dry run, drawn: the agent on one card (each row with an Edit that goes
 *  back to its step), then every row the daemon returned (none hidden) on a
 *  second, then what it will ask, then the charter, folded unless it's
 *  someone's own. */
function ReviewBody({
  plan,
  library,
  tool,
  busy,
  declined,
  onToggle,
  onEdit,
}: {
  plan: HireResult
  library: boolean
  tool: { label: string; tool: string; signIn: SignIn } | null
  busy: boolean
  declined: string[]
  onToggle: (id: string) => void
  onEdit: (step: Step) => void
}): React.JSX.Element {
  const open = asArray<HireResult['questions'][number]>(plan.questions).filter((q) => !q.answered)
  const edit = (to: Step) => (busy ? undefined : () => onEdit(to))
  return (
    <>
      {library ? (
        <Callout tone="warn" className="type-hint" data-testid="create-agent-library-note">
          {CREATE_AGENT_LIBRARY_NOTE}
        </Callout>
      ) : null}
      <ReviewCard testId="create-agent-review-agent">
        <ReviewRow label="Kind" onEdit={edit('pick')} editLabel="Change the kind">
          {plan.template.name}
          <span className="text-[var(--color-text-secondary)]">
            {' '}
            (version {plan.template.version}){library ? `, by ${authorLabel(plan.template.author)}` : ''}
          </span>
        </ReviewRow>
        <ReviewRow label="Name" onEdit={edit('name')} editLabel="Change the name">
          {plan.name} <span className="text-[var(--color-text-secondary)]">@{plan.handle}</span>
        </ReviewRow>
        <ReviewRow label="Folder" onEdit={edit('name')} editLabel="Change the folder">
          <span className="select-text">{plan.path}</span>
        </ReviewRow>
        <ReviewRow label="Tool" onEdit={edit('tool')} editLabel="Change the tool">
          {tool ? (
            <span className="inline-flex flex-wrap items-center gap-x-1.5">
              <span aria-hidden="true" className="inline-flex w-4 justify-center">
                <AgentIcon agent={tool.tool} size={12} />
              </span>
              {tool.label}
              {tool.signIn === 'not_signed_in' ? <span className="text-[var(--color-text-secondary)]">, not signed in yet</span> : null}
            </span>
          ) : (
            <span className="text-[var(--color-text-secondary)]">The server default</span>
          )}
        </ReviewRow>
      </ReviewCard>
      <ReviewSection title={`What ${PRODUCT_NAME} sets up`} testId="create-agent-rows">
        <ReviewItems
          items={asArray<HireResult['rows'][number]>(plan.rows).map((r, i) => {
            const words = hireRowWords(r)
            const base = { key: `${r.step}-${r.target}-${i}`, ...words, testId: 'create-agent-row', step: r.step }
            if (r.step !== 'permission' || r.status === 'suggest') {
              return { ...base, note: rowNote(hireStatusLabel(r.status, r.step), r.reason) }
            }
            return { ...base, ...permissionItem(r, words.what, declined.includes(r.target), busy, onToggle) }
          })}
        />
      </ReviewSection>
      {open.length ? (
        <ReviewSection title="It will ask you" hint="When it starts, in its Thread." testId="create-agent-open-questions">
          <ReviewCard>
            {open.map((q, i) => (
              <ReviewRow key={q.id} label={`Question ${i + 1}`}>
                {q.ask}
              </ReviewRow>
            ))}
          </ReviewCard>
        </ReviewSection>
      ) : null}
      {plan.charter ? <Charter text={plan.charter} startOpen={library} /> : null}
    </>
  )
}

/** A permission row on the Review: a checkbox the person can untick when
 *  the hire would turn it on or ask for it on a card; a disabled one, with
 *  why, when it is on already or stays off. A card says it will ask; one
 *  that stays off says who can turn it on. */
function permissionItem(
  r: HireResult['rows'][number],
  what: string,
  off: boolean,
  busy: boolean,
  onToggle: (id: string) => void,
): { what: React.ReactNode; note: string } {
  const choosable = permissionChoosable(r.status)
  // `skip` is "already on", or (permission cards only) "off on this server".
  const on = choosable ? !off : r.status === 'done' || (r.status === 'skip' && r.reason === 'already on')
  const label = hireStatusLabel(r.status, r.step)
  const note = choosable
    ? off
      ? hireStatusLabel('declined')
      : label
    : r.status === 'refused'
      ? (r.hint ?? rowNote(label, r.reason))
      : r.status === 'skip'
        ? rowNote(label, r.reason)
        : label
  return {
    what: (
      <label className={`inline-flex items-start gap-2 ${choosable && !busy ? 'cursor-pointer' : ''}`}>
        <Checkbox
          checked={on}
          disabled={!choosable || busy}
          onChange={() => onToggle(r.target)}
          data-testid="create-agent-permission-check"
          data-permission={r.target}
          className="mt-0.5 flex-shrink-0"
        />
        <span>{what}</span>
      </label>
    ),
    note,
  }
}

/** The charter, folded under its name (open from the start for someone's own
 *  template), read as formatted text on an inset well. */
function Charter({ text, startOpen }: { text: string; startOpen: boolean }): React.JSX.Element {
  const [open, setOpen] = useState(startOpen)
  const panel = useId()
  return (
    <section data-testid="create-agent-charter" data-open={open ? '' : undefined}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={panel}
        onClick={() => setOpen((v) => !v)}
        data-k2-focus=""
        className="relative -mx-1.5 flex w-[calc(100%+0.75rem)] items-center gap-1.5 px-1.5 py-1 text-left cursor-pointer rounded-[var(--radius-selector)] text-[var(--color-text-primary)] hover:bg-[var(--color-row-hover)] transition-colors"
        data-testid="create-agent-charter-toggle"
      >
        <ChevronRightIcon size={14} className={`flex-shrink-0 text-[var(--color-text-secondary)] transition-transform ${open ? 'rotate-90' : ''}`} />
        <span className="type-row">Charter</span>
        <span className="type-hint text-[var(--color-text-secondary)]">The agent’s standing instructions</span>
      </button>
      <div id={panel} hidden={!open} className="mt-2">
        <Surface role2="inset" chamfer className="px-4 py-3">
          <div data-heading-cap="" data-ui-font="" className="markdown-content type-hint text-[var(--color-text-primary)] select-text [overflow-wrap:anywhere]">
            <Markdown remarkPlugins={[remarkGfm]}>{text}</Markdown>
          </div>
        </Surface>
      </div>
    </section>
  )
}
