// The edit verbs' argv, and how the viewer runs one: always against the
// version on screen (`--base`), always as data (`--json`); first a dry run,
// then the real call told to store exactly the dry run's bytes
// (`--expect <digest>`), so what lands is what was checked. A refusal at
// either step changes nothing. While the server is still reading the file
// it says how long to wait; the call is tried once more after that.

import type { XY } from '../model/types'
import type { EditSource } from './edit-source'
import { localRefusal, type ToolResult } from './tool-result'

/** A coordinate or offset as the verbs read it: plain decimal, never an exponent. */
export function fmtCoord(v: number): string {
  if (!Number.isFinite(v)) return '0'
  if (Number.isInteger(v)) return v === 0 ? '0' : BigInt(v).toString()
  const s = v.toFixed(10).replace(/0+$/, '').replace(/\.$/, '')
  return s === '-0' ? '0' : s
}

/** Move entities by `dx, dy` in the drawing's unit (its word, or `drawing` when it has none). */
export const moveArgv = (addresses: readonly string[], dx: number, dy: number, unit: string): string[] => [
  'entity',
  'move',
  ...addresses,
  '--by',
  `${fmtCoord(dx)},${fmtCoord(dy)}`,
  '--unit',
  unit,
]

/** Turn entities by `deg` degrees (anticlockwise; clockwise is negative) about `about`, in the drawing's unit. */
export const rotateArgv = (addresses: readonly string[], deg: number, about: XY, unit: string): string[] => [
  'entity',
  'rotate',
  ...addresses,
  '--by',
  fmtCoord(Number(deg.toFixed(9))),
  '--about',
  `${fmtCoord(about[0])},${fmtCoord(about[1])}`,
  '--unit',
  unit,
]

/** Scale entities by `factor` (above 0) about `about`, in the drawing's unit. */
export const scaleArgv = (addresses: readonly string[], factor: number, about: XY, unit: string): string[] => [
  'entity',
  'scale',
  ...addresses,
  '--factor',
  fmtCoord(factor),
  '--about',
  `${fmtCoord(about[0])},${fmtCoord(about[1])}`,
  '--unit',
  unit,
]

/** Set one attribute value of a block reference; `nth` (1-based) when its tag repeats. */
export const attribArgv = (insert: string, tag: string, value: string, nth?: number | null): string[] => [
  'attrib',
  'set',
  insert,
  '--tag',
  tag,
  '--value',
  value,
  ...(nth ? ['--nth', String(nth)] : []),
]

/** Set a single-line text's value. */
export const textSetArgv = (text: string, value: string): string[] => ['text', 'set', text, '--value', value]

/** The kit's flags on an edit: the version shown, as data, and a dry run or the digest to store. */
export function withKit(argv: readonly string[], k: { base: number; dryRun?: boolean; expect?: string }): string[] {
  return [...argv, '--base', String(k.base), '--json', ...(k.dryRun ? ['--dry-run'] : []), ...(k.expect ? ['--expect', k.expect] : [])]
}

export type EditOutcome =
  | { kind: 'done'; result: ToolResult }
  | { kind: 'unchanged' }
  | { kind: 'refused'; result: ToolResult }

/** The kit's "still reading the file" refusal: retried once, after the wait it names. */
const PENDING = 'MPC-E713'
const MAX_WAIT_MS = 5000

/** A refusal made here: the dry run came back without the digest to store. */
export const NO_DIGEST = 'no-digest'

const unchanged = (r: ToolResult): boolean => r.ok && r.after === null && /^no change\b/i.test(r.stdout.trim())

export async function runEdit(
  source: EditSource,
  path: string,
  argv: readonly string[],
  base: number,
  wait: (ms: number) => Promise<void> = (ms) => new Promise((r) => setTimeout(r, ms)),
): Promise<EditOutcome> {
  const call = async (a: string[]): Promise<ToolResult> => {
    const r = await source.run(path, a)
    const ms = r.error?.id === PENDING ? Number(r.error.details.retry_after_ms) : NaN
    if (!(ms >= 0 && ms <= MAX_WAIT_MS)) return r
    await wait(ms)
    return source.run(path, a)
  }
  const dry = await call(withKit(argv, { base, dryRun: true }))
  if (!dry.ok) return { kind: 'refused', result: dry }
  if (unchanged(dry)) return { kind: 'unchanged' }
  if (!dry.afterDigest) return { kind: 'refused', result: localRefusal(NO_DIGEST) }
  const real = await call(withKit(argv, { base, expect: dry.afterDigest }))
  if (!real.ok) return { kind: 'refused', result: real }
  if (real.after === null) return { kind: 'unchanged' }
  return { kind: 'done', result: real }
}
