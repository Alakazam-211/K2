// Store selectors must not allocate a fallback inline (0.46.0 React #185).
//
// zustand v5 hooks are useSyncExternalStore underneath: a selector like
// `useXStore((s) => s.list ?? [])` returns a NEW `[]` on every snapshot read
// while `s.list` is missing, React sees the snapshot change forever and
// throws "Maximum update depth exceeded". Default OUTSIDE the selector with
// a module-level constant instead:
//
//   const NO_ITEMS: readonly Item[] = []
//   const items = useXStore((s) => s.list) ?? NO_ITEMS
//
// Source scan over every non-test renderer file. No allowlist.

import { describe, expect, it } from 'vitest'
import { readdirSync, readFileSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const rendererRoot = dirname(fileURLToPath(import.meta.url))

function* walkSources(dir: string): Generator<string> {
  for (const ent of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, ent.name)
    if (ent.isDirectory()) {
      if (ent.name !== 'node_modules') yield* walkSources(p)
    } else if (/\.tsx?$/.test(ent.name) && !/\.test\.tsx?$/.test(ent.name) && !ent.name.endsWith('.d.ts')) {
      yield p
    }
  }
}

// Store hooks and the repo's selector wrappers around zustand's useStore.
const HOOK_CALL = /\b(use[A-Z]\w*Store|useStore|useActivity|useRoomTabs|useRoomProjects)\s*\(/g
const INLINE_FALLBACK = /(\?\?|\|\|)\s*(\[\s*\]|\{\s*\})/

/** Text from `open` (index of the call's `(`) to its matching `)`. */
function callArgs(src: string, open: number): string {
  let depth = 0
  let quote: string | null = null
  for (let i = open; i < src.length; i++) {
    const c = src[i]
    if (quote) {
      if (c === '\\') i++
      else if (c === quote) quote = null
      continue
    }
    if (c === '"' || c === "'" || c === '`') quote = c
    else if (c === '(') depth++
    else if (c === ')' && --depth === 0) return src.slice(open, i + 1)
  }
  throw new Error(`unbalanced call at offset ${open}`)
}

function offenders(): string[] {
  const out: string[] = []
  for (const file of walkSources(rendererRoot)) {
    const src = readFileSync(file, 'utf8')
    for (const m of src.matchAll(HOOK_CALL)) {
      const open = (m.index ?? 0) + m[0].length - 1
      const args = callArgs(src, open)
      if (INLINE_FALLBACK.test(args)) {
        const line = src.slice(0, m.index).split('\n').length
        out.push(`${relative(rendererRoot, file)}:${line}  ${args.replace(/\s+/g, ' ').slice(0, 120)}`)
      }
    }
  }
  return out
}

describe('store selector stability', () => {
  it('the scan finds store selectors to check', () => {
    // Guard the guard: a broken walk or regex would pass vacuously.
    let calls = 0
    for (const file of walkSources(rendererRoot)) calls += [...readFileSync(file, 'utf8').matchAll(HOOK_CALL)].length
    expect(calls).toBeGreaterThan(500)
  })

  it('the scan flags the 0.46.0 Clone dialog selector shape', () => {
    const src = 'const envFiles = useCloneToDialogStore((s) => s.preview?.envFiles ?? [])'
    const open = src.indexOf('(')
    expect(INLINE_FALLBACK.test(callArgs(src, open))).toBe(true)
    const fixed = 'const envFiles = useCloneToDialogStore((s) => s.preview?.envFiles) ?? NO_ENV_FILES'
    expect(INLINE_FALLBACK.test(callArgs(fixed, fixed.indexOf('(')))).toBe(false)
  })

  it('no store selector returns an inline `?? []` / `?? {}` / `|| []` / `|| {}` fallback', () => {
    expect(offenders()).toEqual([])
  })
})
