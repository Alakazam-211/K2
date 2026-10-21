// Test fixtures: the phone contract's examples (permission-cards-phone-
// contract-v1 §11, PRD rev 3.1 §17.11), as the daemon serves them with
// `cards=2`. Field names and values are the contract's, exactly.

type Json = Record<string, unknown>

const ITEM_7F = {
  requestId: 'pr_7f3a2c',
  state: 'pending',
  action: 'compute.use',
  setting: 'Allow compute nodes',
  whatChanges: "Let billing run jobs on this server's compute nodes.",
  direction: 'loosen',
  grant: 'standing',
  scope: 'billing',
  duration: 'until someone turns it off',
  consequences: 'Every active machine; default limits; jobs see only the code billing sends.',
  values: [{ key: 'workspace', value: 'billing' }],
  why: { by: 'billing', text: 'Run the test suite on k2n2-m1; it takes 40 min here' },
  notes: [{ by: 'dana', role: 'member', text: 'I need this for the payroll run', at: 1761060300 }],
  askedFor: 'dana',
  askers: null,
  approvers: { floor: 'admin', label: 'owner or admin', names: ['rosson', 'maya'] },
  proof: { kind: 'click' },
  expiresAt: 1761319200,
  settingsPage: 'workspaces/billing/agent',
  noteBox: { allowed: false, max: 500, mine: null },
  answer: null,
  undo: null,
}

const ROSSON = { login: 'rosson', role: 'owner', via: 'login', mode: 'approve', canAnswer: true, isAsker: false }
const DANA = { login: 'dana', role: 'member', via: 'login', mode: 'wait', canAnswer: false, isAsker: true }
const LEE = { login: 'lee', role: 'member', via: 'login', mode: 'wait', canAnswer: false, isAsker: false }

/** A permission card doc (`action.id = "permission"`). */
export function permissionDoc(o: {
  viewer?: Json
  view?: Json
  choice?: Json
  options?: string[]
  result?: Json | null
}): Json {
  const viewer = o.viewer ?? ROSSON
  const options = o.options ?? (viewer.mode === 'approve' ? ['Allow', 'Deny'] : [])
  return {
    id: 'tc_7f3a2c90',
    kind: 'choice',
    from: 'billing',
    to: 'billing',
    created_at: 1761060000,
    via: 'permission',
    choice: {
      prompt: "billing asks: Let billing run jobs on this server's compute nodes (Allow compute nodes).",
      options: options.map((label) => ({ label })),
      allow_custom: false,
      status: 'pending',
      expiresAt: 1761319200,
      ...(o.choice ?? {}),
      action: {
        id: 'permission',
        params: { request: 'pr_7f3a2c' },
        dedupe: 'compute.use:9b1e40d2',
        ...(o.result ? { result: o.result } : {}),
        view: { v: 1, kind: 'permission', ...ITEM_7F, viewer, ...(o.view ?? {}) },
      },
    },
  }
}

export const VIEWERS = { rosson: ROSSON, dana: DANA, lee: LEE }

/** §11.2: approved from the phone, with "Turn off again". */
export const APPROVED = {
  choice: { status: 'answered', answer: 'Allow', answeredBy: { kind: 'user', login: 'rosson' }, answeredAt: 1761061200 },
  result: { message: 'Allowed. Turn it off again in Settings → Workspaces → billing → Agent.', outcome: 'approved' },
  view: {
    state: 'approved',
    answer: { decision: 'allow', by: 'rosson', via: 'phone', at: 1761061200, reason: null },
    undo: { label: 'Turn off again', until: 1761666000 },
  },
}

/** §11.3: a held App request that needs `app-approve`. */
export const CONFIRM_VIEW = {
  requestId: 'ar_22',
  action: 'apps.approve',
  setting: 'App Approvals',
  whatChanges: "Run shopdev's new service shop (npm start, port 8080) on this server.",
  grant: 'one-time',
  scope: 'request ar_22 (service shop)',
  duration: 'once',
  consequences: "Opens port 8080 to shop's own process outside the container wall's default path.",
  values: [
    { key: 'request', value: 'ar_22' },
    { key: 'command', value: 'npm start' },
    { key: 'port', value: '8080' },
  ],
  why: { by: 'shopdev', text: 'The storefront needs its API process to take checkout calls' },
  notes: [],
  askedFor: null,
  approvers: { floor: 'owner', label: 'owner', names: ['rosson'] },
  proof: { kind: 'confirm', action: 'app-approve', secretId: 'ar_22' },
  settingsPage: 'app-approvals',
}

/** §11.12: a batch, one item needs a confirm. */
export function batchDoc(o: { canAnswerAll?: boolean; items?: Json[]; state?: string; viewer?: Json } = {}): Json {
  const base = (requestId: string, extra: Json): Json => ({
    ...ITEM_7F,
    requestId,
    notes: [],
    askedFor: null,
    why: { by: 'billing', text: 'Set up the App' },
    approvers: { floor: 'owner', label: 'owner', names: ['rosson'] },
    ...extra,
  })
  const items = o.items ?? [
    base('pr_a1', { action: 'apps.build', setting: 'Build and run Apps', whatChanges: 'Let billing build and run Apps.' }),
    base('pr_a2', {
      action: 'apps.core',
      setting: 'App cores',
      whatChanges: "Let billing's Apps use the Database core.",
      values: [
        { key: 'workspace', value: 'billing' },
        { key: 'core', value: 'db' },
      ],
    }),
    base('ar_23', {
      action: 'apps.approve',
      setting: 'App Approvals',
      whatChanges: "Let billing's App reach api.stripe.com.",
      grant: 'one-time',
      duration: 'once',
      proof: { kind: 'confirm', action: 'app-approve', secretId: 'ar_23' },
      values: [{ key: 'request', value: 'ar_23' }],
    }),
  ]
  return {
    id: 'tc_b4a1c0de',
    kind: 'choice',
    from: 'billing',
    to: 'billing',
    created_at: 1761060000,
    via: 'permission',
    choice: {
      prompt: 'billing asks for 3 permissions.',
      options: [{ label: 'Allow all' }, { label: 'Deny all' }],
      allow_custom: false,
      status: 'pending',
      expiresAt: 1761319200,
      action: {
        id: 'permission.batch',
        params: { requests: items.map((i) => i.requestId) },
        dedupe: 'batch:sess_9:owner',
        view: {
          v: 1,
          kind: 'batch',
          state: o.state ?? 'pending',
          viewer: o.viewer ?? { ...ROSSON, canAnswerAll: o.canAnswerAll ?? false },
          approvers: { floor: 'owner', label: 'owner', names: ['rosson'] },
          expiresAt: 1761319200,
          items,
        },
      },
    },
  }
}

/** §11.11: the asker's fallback on a build without `cards=2`. */
export const FALLBACK_TEXT = {
  id: 'tc_7f3a2c90',
  kind: 'text',
  from: 'billing',
  to: 'billing',
  created_at: 1761060000,
  via: 'permission',
  body: 'Asked an admin to turn on Allow compute nodes for you. You can keep working with billing meanwhile.',
}

/** §17.6: the approver's click-action fallback (no `view`). */
export const FALLBACK_CHOICE = {
  id: 'tc_7f3a2c90',
  kind: 'choice',
  from: 'billing',
  to: 'billing',
  created_at: 1761060000,
  via: 'permission',
  choice: {
    prompt: "billing asks: Let billing run jobs on this server's compute nodes (Allow compute nodes). billing says: Run the test suite",
    options: [{ label: 'Allow' }, { label: 'Deny' }],
    allow_custom: false,
    status: 'pending',
    action: { id: 'permission', params: { request: 'pr_7f3a2c' } },
  },
}
