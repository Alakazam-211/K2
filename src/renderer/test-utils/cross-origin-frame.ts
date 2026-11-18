// A stand-in for a sandboxed iframe's window, as the desktop app's webview
// (WebKit) hands it to the page: an opaque-origin WindowProxy. Reading any
// property outside the HTML spec's cross-origin allowlist throws a
// SecurityError ("Blocked a frame at … from accessing a cross-origin frame.
// The frame being accessed is sandboxed and lacks the "allow-same-origin"
// flag."). React 19's dev build walks a re-rendered component's props to log
// what changed (logComponentRender → addObjectDiffToProperties →
// addValueToProperties reads `value.$$typeof`), so a prop that holds such a
// window, or reaches one within a few levels (the top `window` does: its
// indexed properties are its frames), throws in the commit and the UI dies.

/** The names HTML lets a page read on a cross-origin window
 *  (CrossOriginProperties, html.spec.whatwg.org §7.2.3.1), plus the symbols
 *  [[Get]] answers with undefined. */
const ALLOWED = new Set<PropertyKey>([
  'window', 'self', 'location', 'close', 'closed', 'focus', 'blur', 'frames', 'length', 'top', 'opener', 'parent', 'postMessage',
  'then', Symbol.toStringTag, Symbol.hasInstance, Symbol.isConcatSpreadable,
])

export const CROSS_ORIGIN_READ = 'cross-origin frame read'

/** A fake opaque-origin WindowProxy: allowed names answer, every other read
 *  throws a SecurityError naming the property. */
export function crossOriginWindow(): Window {
  const target = Object.create(null) as Record<PropertyKey, unknown>
  const self: Window = new Proxy(target, {
    get(_t, key) {
      if (!ALLOWED.has(key)) throw new DOMException(`${CROSS_ORIGIN_READ}: ${String(key)}`, 'SecurityError')
      if (key === 'window' || key === 'self' || key === 'frames') return self
      if (key === 'length') return 0
      if (key === 'closed') return false
      if (key === 'postMessage' || key === 'focus' || key === 'blur' || key === 'close') return () => undefined
      return undefined
    },
    has: (_t, key) => ALLOWED.has(key),
    ownKeys: () => [],
    getOwnPropertyDescriptor: () => undefined,
    getPrototypeOf: () => null,
    set: (_t, key) => {
      throw new DOMException(`${CROSS_ORIGIN_READ}: set ${String(key)}`, 'SecurityError')
    },
  }) as unknown as Window
  return self
}

/** Puts a sandboxed frame on the page for one test, as WebKit shows it: the
 *  top window gets an enumerable own `0` that is the frame's window, and
 *  every iframe's `contentWindow` is an opaque-origin window. Also gives
 *  `console.timeStamp`, which React's dev build checks (when react-dom loads)
 *  before it logs component renders; import react-dom after this runs.
 *  Returns the undo. */
export function withCrossOriginFrame(): () => void {
  const xo = crossOriginWindow()
  const c = console as unknown as { timeStamp?: () => void }
  if (typeof c.timeStamp !== 'function') c.timeStamp = () => {}
  const proto = HTMLIFrameElement.prototype
  const before = Object.getOwnPropertyDescriptor(proto, 'contentWindow')
  Object.defineProperty(window, '0', { get: () => xo, enumerable: true, configurable: true })
  Object.defineProperty(proto, 'contentWindow', { get: () => xo, configurable: true })
  return () => {
    delete (window as unknown as Record<string, unknown>)['0']
    if (before) Object.defineProperty(proto, 'contentWindow', before)
  }
}
