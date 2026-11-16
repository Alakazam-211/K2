// Map.prototype.getOrInsert and getOrInsertComputed (the TC39 "upsert"
// proposal), which pdf.js 5 calls without a fallback in its main build and
// its worker. WKWebView on the macOS versions K2 supports may not have them,
// and without them no PDF opens. Added only where missing, non-enumerable,
// to Map and WeakMap, as the proposal specifies.

type Upsert = {
  has(k: unknown): boolean
  get(k: unknown): unknown
  set(k: unknown, v: unknown): unknown
}

export function installMapUpsert(C: { prototype: object }): void {
  const proto = C.prototype as Record<string, unknown>
  const define = (name: string, fn: (this: Upsert, ...a: never[]) => unknown): void => {
    if (typeof proto[name] === 'function') return
    Object.defineProperty(proto, name, { value: fn, writable: true, configurable: true, enumerable: false })
  }
  define('getOrInsert', function (this: Upsert, key: never, value: never) {
    if (this.has(key)) return this.get(key)
    this.set(key, value)
    return value
  })
  define('getOrInsertComputed', function (this: Upsert, key: never, compute: (k: unknown) => unknown) {
    if (typeof compute !== 'function') throw new TypeError('getOrInsertComputed: callback is not a function')
    if (this.has(key)) return this.get(key)
    const value = compute(key)
    this.set(key, value)
    return value
  })
}
