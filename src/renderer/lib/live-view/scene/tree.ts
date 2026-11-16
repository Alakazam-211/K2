// The outliner's tree: collections nested by their parent (the view index
// carries it since BLEND main 0c552c1a), siblings in position order; objects
// in their container, nested under a parent in the same container. A parent
// the tab can't use (not in the file, a cycle, or nested deeper than
// MAX_NEST) is refused: that collection shows at the top level and the
// refusal goes to the picture's details only. Then the flat rows the list
// draws (virtualised), with search keeping matches and their collections.

import { CONT, UNITS, collOf, objOf, type SceneColl, type SceneObj } from './fold'
import type { Doc } from './types'

/** The mock's guess; the kit's own CONTAINER_DEPTH_MAX isn't pinned (PRD §6). */
export const MAX_NEST = 32

export interface Refused {
  addr: string
  why: 'unknown' | 'cycle' | 'too deep'
  parent: string
}

export interface SceneTree {
  colls: Record<string, SceneColl>
  objs: Record<string, SceneObj>
  kids: Record<string, string[]>
  objsIn: Record<string, SceneObj[]>
  roots: string[]
  order: string[]
  refused: Refused[]
}

export function sceneTree(D: Doc): SceneTree {
  const colls = (D.lists[CONT] ?? []).map((a) => collOf(D, a))
  const objs = (D.lists[UNITS] ?? []).map((a) => objOf(D, a))
  const T: SceneTree = { colls: {}, objs: {}, kids: {}, objsIn: {}, roots: [], order: objs.map((o) => o.addr), refused: [] }
  const keys = new Set(D.lists[CONT] ?? [])
  const par: Record<string, string | null> = {}
  for (const c of colls) par[c.addr] = c.parent || null
  for (const c of colls) {
    const p = par[c.addr]
    if (!p) continue
    if (!keys.has(p)) {
      T.refused.push({ addr: c.addr, why: 'unknown', parent: p })
      par[c.addr] = null
      continue
    }
    let x: string | null = p
    let d = 1
    const seen = new Set([c.addr])
    while (x) {
      if (seen.has(x)) {
        T.refused.push({ addr: c.addr, why: 'cycle', parent: p })
        par[c.addr] = null
        break
      }
      seen.add(x)
      x = par[x]
      if (++d > MAX_NEST) {
        T.refused.push({ addr: c.addr, why: 'too deep', parent: p })
        par[c.addr] = null
        break
      }
    }
  }
  for (const c of colls) {
    T.colls[c.addr] = c
    const p = par[c.addr]
    if (p) (T.kids[p] = T.kids[p] ?? []).push(c.addr)
    else T.roots.push(c.addr)
  }
  for (const o of objs) {
    T.objs[o.addr] = o
    ;(T.objsIn[o.container] = T.objsIn[o.container] ?? []).push(o)
  }
  return T
}

export type Row = { t: 'coll'; addr: string; depth: number; open: boolean } | { t: 'obj'; addr: string; depth: number } | { t: 'more'; addr: string; n: number; depth: number } | { t: 'skel'; addr: string; depth: number }

export interface Rows extends Array<Row> {
  hits: number
}

export function flatRows(T: SceneTree, collapsed: ReadonlySet<string>, q: string | null | undefined): Rows {
  const rows = [] as unknown as Rows
  const ql = (q || '').trim().toLowerCase()
  const match = (o: SceneObj): boolean => !ql || o.name.toLowerCase().includes(ql) || o.addr.toLowerCase() === ql
  const memo: Record<string, boolean> = {}
  const sub = (c: string): boolean => (c in memo ? memo[c] : (memo[c] = (T.objsIn[c] ?? []).some(match) || (T.kids[c] ?? []).some(sub)))
  let hits = 0
  const visit = (c: string, depth: number): void => {
    if (ql && !sub(c)) return
    const open = ql ? true : !collapsed.has(c)
    rows.push({ t: 'coll', addr: c, depth, open })
    if (!open) return
    for (const k of T.kids[c] ?? []) visit(k, depth + 1)
    const inC = T.objsIn[c] ?? []
    const here = new Set(inC.map((o) => o.addr))
    const childOf: Record<string, SceneObj[]> = {}
    for (const o of inC) if (o.parent && here.has(o.parent)) (childOf[o.parent] = childOf[o.parent] ?? []).push(o)
    const put = (o: SceneObj, d: number): void => {
      if (match(o)) {
        rows.push({ t: 'obj', addr: o.addr, depth: ql ? depth + 1 : d })
        if (ql) hits++
      }
      for (const ch of childOf[o.addr] ?? []) put(ch, d + 1)
    }
    for (const o of inC.filter((x) => !(x.parent && here.has(x.parent)))) put(o, depth + 1)
  }
  for (const r of T.roots) visit(r, 0)
  rows.hits = hits
  return rows
}

/** The collections an object (or collection) sits in, innermost first. */
export function ancestorsOf(T: SceneTree, a: string): string[] {
  const out: string[] = []
  const o = T.objs[a]
  let c: string | null = o ? o.container : a
  while (c && T.colls[c]) {
    out.push(c)
    c = T.colls[c].parent
  }
  return out
}

export const countDeep = (T: SceneTree, c: string): number => (T.objsIn[c] ?? []).length + (T.kids[c] ?? []).reduce((n, k) => n + countDeep(T, k), 0)
