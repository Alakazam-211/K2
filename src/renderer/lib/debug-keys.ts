// Developer switches kept in localStorage.
//
// Each switch has a `K5_` key, read first, and keeps its older `K2SO_`
// key, read after it. A key that is set wins over the one after it
// whatever its value. Reading never throws: where there is no
// localStorage (tests, a locked-down webview) the switch is unset.

/** The keys one switch is read under, newest first. */
export function debugKeyNames(name: string): [string, string] {
  return [`K5_${name}`, `K2SO_${name}`]
}

/** The value of a switch, or null when it is not set under either key. */
export function readDebugKey(name: string): string | null {
  try {
    if (typeof localStorage === 'undefined') return null
    for (const key of debugKeyNames(name)) {
      const value = localStorage.getItem(key)
      if (value !== null) return value
    }
    return null
  } catch {
    return null
  }
}
