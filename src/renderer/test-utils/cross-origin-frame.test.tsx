// @vitest-environment jsdom
//
// The stand-in for a sandboxed frame's window (cross-origin-frame.ts) and the
// dev-mode crash it models. React 19's dev build logs each re-rendered
// component's changed props (logComponentRender → addObjectDiffToProperties
// → addValueToProperties, react-dom-client.development.js; the production
// build has none of it). That walk reads `value.$$typeof` on every value and
// for-ins objects a few levels deep, the top `window` included, whose own
// index keys are its frames. In WebKit a read on a sandboxed frame's window
// throws "Sandbox access violation: Blocked a frame … lacks the
// "allow-same-origin" flag", the passive-effects commit aborts and the UI
// stops answering clicks. So no prop or state may hold a frame's window, the
// top window, or anything that reaches one (a React event's `view`).

import { afterEach, describe, expect, it } from 'vitest'
import { createRequire } from 'node:module'
import { CROSS_ORIGIN_READ, crossOriginWindow, withCrossOriginFrame } from './cross-origin-frame'

const restore: (() => void)[] = []
afterEach(() => {
  while (restore.length) restore.pop()!()
})

function Child({ v }: { v: unknown }): React.JSX.Element {
  return <div>{typeof v}</div>
}

/** Mount, then re-render with `next`: the second commit logs the prop diff.
 *  Returns what that commit threw. A throw there leaves that react-dom copy
 *  wedged ("Should not already be working": the dead UI), so every call
 *  loads a fresh copy of react and react-dom (dev builds). */
async function rerenderWith(next: unknown): Promise<unknown> {
  const req = createRequire(import.meta.url)
  for (const k of Object.keys(req.cache)) {
    if (/[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/.test(k)) delete req.cache[k]
  }
  const { act } = req('react') as typeof import('react')
  const { createRoot } = req('react-dom/client') as typeof import('react-dom/client')
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const el = document.createElement('div')
  document.body.append(el)
  const root = createRoot(el)
  await act(async () => root.render(<Child v={null} />))
  let thrown: unknown = null
  try {
    await act(async () => root.render(<Child v={next} />))
  } catch (e) {
    thrown = e
  }
  if (thrown === null) await act(async () => root.unmount())
  el.remove()
  return thrown
}

describe('cross-origin frame stand-in', () => {
  it('answers the cross-origin allowlist and throws a SecurityError on anything else', () => {
    const w = crossOriginWindow()
    expect(() => w.postMessage('x', '*')).not.toThrow()
    expect(w.window).toBe(w)
    expect(Object.prototype.toString.call(w)).toBe('[object Object]')
    expect(() => (w as unknown as { $$typeof: unknown }).$$typeof).toThrow(CROSS_ORIGIN_READ)
    expect(() => w.document).toThrow(/SecurityError|cross-origin/)
  })

  // The two tests below prove the helper reproduces the bug under React's dev
  // logging, so the component tests that use it are real checks.
  it("React dev's prop logging throws on a prop that holds a frame's window", async () => {
    restore.push(withCrossOriginFrame())
    const err = await rerenderWith({ frame: crossOriginWindow() })
    expect(String(err)).toContain(CROSS_ORIGIN_READ)
  })

  it('and on a prop that holds the top window while a sandboxed frame is on the page', async () => {
    restore.push(withCrossOriginFrame())
    const err = await rerenderWith({ target: window })
    expect(String(err)).toContain(CROSS_ORIGIN_READ)
  })

  it('a plain id or a function closing over the window is fine', async () => {
    restore.push(withCrossOriginFrame())
    const w = crossOriginWindow()
    expect(await rerenderWith({ frameId: 'zen:g1/w1', post: () => w.postMessage('hi', '*') })).toBeNull()
  })
})
