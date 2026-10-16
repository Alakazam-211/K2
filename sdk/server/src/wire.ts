// The wire: catalog arguments → a request, and a live answer → the
// catalog's closed shape (runtime S23, room-scope RS7).
//
// - Argument names map to wire names through the row's `params`.
// - GET sends every argument in the query; POST sends JSON (the row's
//   `query` arguments ride the query string instead).
// - An answer is projected onto the row's schema: the fields it names are
//   read under their wire names (`responseNames`: relPath ← path), every
//   other field is dropped (banned host details such as session ids never
//   reach App code), and `json`/map values pass through untouched.

import type { AppVerbRow, Schema } from './generated/verbs.ts'

type Http = NonNullable<AppVerbRow['http']>

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

/** One query value as K2 reads it (arrays comma-joined, booleans true/false). */
export function queryValue(v: unknown): string | null {
  if (v === undefined || v === null) return null
  if (Array.isArray(v)) return v.map((x) => String(x)).join(',')
  if (typeof v === 'object') return JSON.stringify(v)
  return String(v)
}

export interface WireRequest {
  method: 'GET' | 'POST'
  /** Path and query, e.g. `/cli/thread?addr=sales`. */
  pathAndQuery: string
  /** The JSON body (POST only). */
  body: string | null
}

/** The request for `args` on an http row. */
export function buildRequest(http: Http, args: Record<string, unknown> | undefined): WireRequest {
  const q = new URLSearchParams()
  const body: Record<string, unknown> = {}
  for (const [name, value] of Object.entries(args ?? {})) {
    if (value === undefined) continue
    const wire = http.params[name] ?? name
    if (http.method === 'GET' || http.query.includes(name)) {
      const qv = queryValue(value)
      if (qv !== null) q.set(wire, qv)
    } else {
      body[wire] = value
    }
  }
  const qs = q.toString()
  return {
    method: http.method,
    pathAndQuery: qs ? `${http.route}?${qs}` : http.route,
    body: http.method === 'POST' ? JSON.stringify(body) : null,
  }
}

function typesOf(s: Schema): string[] {
  const t = s.type
  if (typeof t === 'string') return [t]
  if (Array.isArray(t)) return t.filter((x): x is string => typeof x === 'string')
  return []
}

/**
 * `value` read through `schema`: object properties under their wire names
 * (`names`: catalog → wire, at any depth), unnamed fields dropped, arrays
 * item by item, maps and `json` values as they are.
 */
export function project(schema: Schema, value: unknown, names: Readonly<Record<string, string>>): unknown {
  if (value === null || value === undefined) return value
  const types = typesOf(schema)
  if (types.includes('json')) return value
  if (Array.isArray(value)) {
    const items = schema.items as Schema | undefined
    return items ? value.map((x) => project(items, x, names)) : value
  }
  if (isRecord(value)) {
    const props = schema.properties as Record<string, Schema> | undefined
    if (props) {
      const out: Record<string, unknown> = {}
      for (const [name, sub] of Object.entries(props)) {
        const wire = names[name] ?? name
        if (Object.prototype.hasOwnProperty.call(value, wire)) out[name] = project(sub, value[wire], names)
      }
      return out
    }
    const map = schema.additionalProperties
    if (isRecord(map)) {
      const out: Record<string, unknown> = {}
      for (const [k, v] of Object.entries(value)) out[k] = project(map as Schema, v, names)
      return out
    }
  }
  return value
}
