/**
 * Coerce unknown JSON into a real array.
 *
 * Remote hosts (and the web shim) sometimes return a non-array body for
 * list endpoints — empty string → `undefined` after parse, an error
 * envelope object, a wrapped `{ items: [...] }` shape, etc. Spreading
 * that with `[...x]` crashes the whole SPA under Bun/WebKit:
 *   "Spread syntax requires ...iterable[Symbol.iterator] to be a function"
 * which surfaces as AppErrorBoundary ("Something went wrong rendering K2").
 *
 * Prefer this at every daemon list boundary before `.map` / spread / sort.
 * Soft-empty on bad shape so panels degrade instead of black-screening.
 */
export function asArray<T = unknown>(raw: unknown): T[] {
  if (Array.isArray(raw)) return raw as T[]
  if (
    raw !== null &&
    typeof raw === 'object' &&
    Array.isArray((raw as { items?: unknown }).items)
  ) {
    return (raw as { items: T[] }).items
  }
  if (
    raw !== null &&
    typeof raw === 'object' &&
    Array.isArray((raw as { entries?: unknown }).entries)
  ) {
    return (raw as { entries: T[] }).entries
  }
  if (
    raw !== null &&
    typeof raw === 'object' &&
    Array.isArray((raw as { users?: unknown }).users)
  ) {
    return (raw as { users: T[] }).users
  }
  return []
}
