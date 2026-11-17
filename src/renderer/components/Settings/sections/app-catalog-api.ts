// Settings → Apps → Add an App (prd-one-click-suite-apps-v1 R1–R5):
// the thin client over the daemon's catalog and install job. Every answer
// comes from the window's server (`primaryScope()`); the job runs in the
// daemon, so closing the sheet or the window never stops it.
//
// Routes: GET /cli/app/catalog, GET /cli/app/catalog/check-address,
// POST /cli/app/catalog/install, GET /cli/app/catalog/job, POST
// /cli/app/catalog/retry.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

/** The daemon's words come in two parts: `text`, a plain sentence for
 *  people, and `hint`, the command an agent runs (null when there is none).
 *  People never see the command. A server older than this split sent only
 *  `hint`, the sentence with its command. */
export interface Worded {
  text?: string | null
  hint?: string | null
}

export interface CatalogReason extends Worded {
  code: string
  text: string
}

export interface CatalogApproval {
  kind: 'boxService' | 'cap'
  name: string
  why?: string
}

export interface CatalogStep {
  step: string
  state: string
  note?: string | null
}

export interface CatalogJob {
  id: string
  entry: string
  version: string
  variant: string
  app: string
  address: { kind: string; label: string; url: string | null }
  step: string
  state: 'running' | 'waiting' | 'failed' | 'done'
  steps: CatalogStep[]
  waiting?: { reason: string | null; requests: string[] } & Worded
  error?: { code: string; retry: boolean } & Worded
  /** The CLI's lines, each with its command. */
  next?: string[]
  /** What to do next: `text` for people, `hint` the command apart. */
  nextSteps?: Array<{ text: string; hint: string | null }>
}

export interface CatalogEntry {
  entry: string
  name: string
  publisher: { id: string; name: string; source: string }
  summary: string
  license: string
  /** "Verified by K2" only once a pinned, published version exists; null
   *  until then (no badge). */
  verified: string | null
  family: string
  defaultAddress: { label: string; url: string | null }
  onePerBox: boolean
  version: string | null
  variant: string | null
  willDo: string[]
  approves: CatalogApproval[]
  canDeploy: boolean
  reasons: CatalogReason[]
  alreadyDeployed: string | null
  installed: Array<{ app: string; version: string; variant: string }>
  jobs: CatalogJob[]
}

export interface AddressCheck {
  ok: boolean
  app: string
  address?: { kind: string; label: string; url: string | null }
  error?: { code: string; hint: string; text?: string; suggestion?: string; holder?: string }
  tier?: { pro: boolean | null; hint?: string }
}

/** The steps in the daemon's order (vs-live A5), with the sheet's words. */
export const STEP_LABELS: ReadonlyArray<[string, string]> = [
  ['download', 'Download'],
  ['check', 'Check'],
  ['install', 'Install'],
  ['address', 'Address'],
  ['deploy', 'Deploy'],
  ['approve', 'Approve'],
  ['sew_setup', 'Chat setup'],
  ['healthy', 'Healthy'],
  ['ready', 'Ready'],
]

export function stepLabel(step: string): string {
  return STEP_LABELS.find(([id]) => id === step)?.[1] ?? step
}

/** A wait in words (R4): what it waits for and who moves it on. The
 *  server's own hint (CLI steps, open questions) stays out of the sheet for
 *  the waits it names here. */
export function waitText(job: CatalogJob): string | null {
  if (job.state !== 'waiting' || !job.waiting) return null
  switch (job.waiting.reason) {
    case 'waiting_approval':
      return 'Waiting for the owner to approve what this App uses. The deploy goes on by itself once it’s approved.'
    case 'waiting_sew_rights':
      return 'Waiting for permission to set up chat and voice. The owner can turn on “Manage chat and voice” for the agent that looks after it, or deploy it themselves; the deploy goes on by itself then.'
    default:
      return 'Waiting for the server. The deploy goes on by itself.'
  }
}

/** The plain sentence of a daemon's words: `text` when the server splits
 *  them, else (an older server) its `hint`. Never the command alone. */
export function plainText(w: Worded | null | undefined): string | null {
  if (!w) return null
  if (w.text !== undefined) return w.text ?? null
  return w.hint ?? null
}

export function isActive(job: CatalogJob | null | undefined): boolean {
  return job != null && (job.state === 'running' || job.state === 'waiting')
}

export async function fetchCatalog(): Promise<CatalogEntry[]> {
  const v = await daemonCliGet<{ entries?: CatalogEntry[] }>(primaryScope(), 'app/catalog')
  return Array.isArray(v.entries) ? v.entries : []
}

export async function checkAddress(entry: string, address: string, workspace: string | null): Promise<AddressCheck> {
  return daemonCliGet<AddressCheck>(primaryScope(), 'app/catalog/check-address', {
    entry,
    address,
    workspace: workspace ?? undefined,
  })
}

export async function startInstall(entry: string, address: string, workspace: string | null): Promise<CatalogJob> {
  const v = await daemonCliPost<{ job: CatalogJob }>(primaryScope(), 'app/catalog/install', {
    entry,
    address: address.trim() || undefined,
    workspace: workspace ?? undefined,
  })
  return v.job
}

export async function fetchJob(id: string): Promise<CatalogJob> {
  const v = await daemonCliGet<{ job: CatalogJob }>(primaryScope(), 'app/catalog/job', { id })
  return v.job
}

export async function retryJob(id: string): Promise<CatalogJob> {
  const v = await daemonCliPost<{ job: CatalogJob }>(primaryScope(), 'app/catalog/retry', { job: id })
  return v.job
}
