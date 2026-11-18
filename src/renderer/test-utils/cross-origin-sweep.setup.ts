// Setup for `bun run test:cross-origin` (vitest.cross-origin.config.ts): every
// jsdom test runs as if a sandboxed frame were on the page, with React's dev
// render logging on. A component whose props hold the top window, a frame's
// window, or anything that reaches one within a few levels (a CodeMirror
// EditorView: `observer.win`; a React event: `view`) then fails with
// "cross-origin frame read: …", the way the desktop app's dev build dies in
// WebKit. Not part of `vitest run`: render logging slows every test down.
// iframes' own `contentWindow` stays jsdom's (tests that drive a same-origin
// frame keep working); `window[0]` is the opaque one.
import { crossOriginWindow } from './cross-origin-frame'

const c = console as unknown as { timeStamp?: () => void }
if (typeof c.timeStamp !== 'function') c.timeStamp = () => {}
if (typeof window !== 'undefined' && typeof HTMLIFrameElement !== 'undefined') {
  const xo = crossOriginWindow()
  Object.defineProperty(window, '0', { get: () => xo, enumerable: true, configurable: true })
}
