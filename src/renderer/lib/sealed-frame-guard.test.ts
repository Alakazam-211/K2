// @vitest-environment jsdom
//
// The sealed-frame guard (src/shared/sealed-frame-guard.js): in a subframe
// it removes WebRTC, link resource hints and `<a ping>`; in the top frame it
// does nothing. jsdom has no WebRTC, so each test puts stand-in constructors
// on the frame window first and runs the guard text against that window.
// The real-engine check is scripts/sealed-frame-check/wkwebview-caps.swift.
import { afterEach, describe, expect, it } from 'vitest'
import { SEALED_FRAME_GUARD } from './sealed-frame-guard'

const WEBRTC = [
  'RTCPeerConnection',
  'webkitRTCPeerConnection',
  'RTCDataChannel',
  'RTCSessionDescription',
  'RTCIceCandidate',
  'RTCRtpSender',
  'RTCRtpReceiver',
  'RTCRtpTransceiver',
  'RTCDtlsTransport',
  'RTCIceTransport',
  'RTCSctpTransport',
  'MediaStreamTrackProcessor',
] as const

type Win = Window & Record<string, unknown>

function withStandIns(win: Win): void {
  for (const name of WEBRTC) {
    Object.defineProperty(win, name, {
      value: function StandIn() {},
      writable: true,
      configurable: true,
      enumerable: false,
    })
  }
}

/** Run the guard text with `window` bound to `win`, as the engine would. */
function runGuard(win: Win): void {
  new Function('window', SEALED_FRAME_GUARD)(win)
}

function subframe(): { frame: HTMLIFrameElement; win: Win; doc: Document } {
  const frame = document.createElement('iframe')
  document.body.appendChild(frame)
  const win = frame.contentWindow as Win | null
  if (!win) throw new Error('jsdom gave the iframe no window')
  const doc = win.document
  if (!doc.body) doc.documentElement.appendChild(doc.createElement('body'))
  // Booleans only: a failing diff of two windows is too big to report.
  expect(win.top === win.self).toBe(false)
  return { frame, win, doc }
}

const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0))

afterEach(() => {
  document.body.innerHTML = ''
})

describe('sealed-frame guard: top frame', () => {
  it('does nothing in the top frame: WebRTC stays and nothing is patched', () => {
    const win = window as unknown as Win
    withStandIns(win)
    const setAttr = Element.prototype.setAttribute
    runGuard(win)
    for (const name of WEBRTC) {
      expect(typeof win[name], name).toBe('function')
      expect(Object.getOwnPropertyDescriptor(win, name)?.configurable, name).toBe(true)
    }
    expect(Element.prototype.setAttribute).toBe(setAttr)
    expect(Object.prototype.hasOwnProperty.call(win, '__k2SealedFrameGuard')).toBe(false)
    const l = document.createElement('link')
    l.rel = 'preconnect'
    expect(l.getAttribute('rel')).toBe('preconnect')
    for (const name of WEBRTC) delete win[name]
  })
})

describe('sealed-frame guard: subframe', () => {
  it('removes every WebRTC constructor, and they cannot be put back', () => {
    const { win } = subframe()
    withStandIns(win)
    runGuard(win)
    for (const name of WEBRTC) {
      expect(typeof win[name], name).toBe('undefined')
      const d = Object.getOwnPropertyDescriptor(win, name)
      expect(d, name).toBeDefined()
      expect(d!.configurable, name).toBe(false)
      expect(d!.writable, name).toBe(false)
      // Test modules are strict: assignment to a read-only property throws.
      expect(() => {
        win[name] = function Back() {}
      }, name).toThrow(TypeError)
      expect(() => Object.defineProperty(win, name, { value: function Back() {} }), name).toThrow(TypeError)
      expect(() => delete win[name], name).toThrow(TypeError)
      expect(typeof win[name], name).toBe('undefined')
    }
  })

  it('removes them even when the engine never had them (nothing to define later)', () => {
    const { win } = subframe()
    runGuard(win)
    expect(typeof win.RTCPeerConnection).toBe('undefined')
    expect(() => {
      win.RTCPeerConnection = function Back() {}
    }).toThrow(TypeError)
  })

  it('running twice is a no-op', () => {
    const { win } = subframe()
    withStandIns(win)
    runGuard(win)
    expect(() => runGuard(win)).not.toThrow()
    expect(typeof win.RTCPeerConnection).toBe('undefined')
  })

  it('link hints: rel never takes a hint token by property, attribute or relList', () => {
    const { win, doc } = subframe()
    runGuard(win)
    for (const hint of ['dns-prefetch', 'preconnect', 'prefetch', 'preload', 'modulepreload', 'PreConnect']) {
      const a = doc.createElement('link')
      a.rel = hint
      expect(a.getAttribute('rel'), hint).toBe('')
      const b = doc.createElement('link')
      b.setAttribute('rel', `${hint} stylesheet`)
      expect(b.getAttribute('rel'), hint).toBe('stylesheet')
      const c = doc.createElement('link')
      c.setAttributeNS(null, 'rel', hint)
      expect(c.getAttribute('rel'), hint).toBe('')
      const d = doc.createElement('link')
      d.relList.add('icon', hint)
      expect(d.relList.contains(hint), hint).toBe(false)
      expect(d.relList.contains('icon')).toBe(true)
      expect(d.relList.toggle(hint)).toBe(false)
      d.relList.value = `${hint} icon`
      expect(d.getAttribute('rel'), hint).toBe('icon')
    }
    // Other elements and other rels are untouched.
    const s = doc.createElement('link')
    s.rel = 'stylesheet'
    expect(s.getAttribute('rel')).toBe('stylesheet')
    const div = doc.createElement('div')
    div.setAttribute('rel', 'preconnect')
    expect(div.getAttribute('rel')).toBe('preconnect')
    expect(div.classList.toggle('preload')).toBe(true)
  })

  it('link hints: an existing hint link loses its href and is dropped', () => {
    const { win, doc } = subframe()
    runGuard(win)
    const holder = doc.createElement('div')
    holder.innerHTML = '<link rel="preconnect">' // detached: not observed yet
    const link = holder.firstChild as HTMLLinkElement
    link.href = 'https://example.test/'
    expect(link.getAttribute('href')).toBeNull()
    expect(link.parentNode).toBeNull()
  })

  it('link hints the parser or innerHTML adds are removed', async () => {
    const { win, doc } = subframe()
    runGuard(win)
    doc.body.innerHTML =
      '<link rel="dns-prefetch" href="//example.test"><link rel="modulepreload" href="m.js"><link rel="stylesheet" href="s.css">'
    await flush()
    const rels = [...doc.querySelectorAll('link')].map((l) => l.getAttribute('rel'))
    expect(rels).toEqual(['stylesheet'])
  })

  it('a hint already in the document when the guard starts is removed', () => {
    const { win, doc } = subframe()
    doc.head.innerHTML = '<link rel="prefetch" href="x">'
    runGuard(win)
    expect(doc.querySelectorAll('link')).toHaveLength(0)
  })

  it('ping: never set on a or area, by property, attribute or markup', async () => {
    const { win, doc } = subframe()
    runGuard(win)
    const a = doc.createElement('a')
    a.ping = 'https://example.test/p'
    expect(a.getAttribute('ping')).toBeNull()
    a.setAttribute('ping', 'https://example.test/p')
    expect(a.getAttribute('ping')).toBeNull()
    const area = doc.createElement('area')
    area.setAttribute('PING', 'p')
    expect(area.getAttribute('ping')).toBeNull()
    doc.body.innerHTML = '<a href="#x" ping="p">x</a><area ping="p">'
    await flush()
    expect(doc.querySelectorAll('[ping]')).toHaveLength(0)
    expect(doc.querySelector('a')!.getAttribute('href')).toBe('#x')
  })

  it('a nested srcdoc frame gets the guard as its first script (after a doctype)', async () => {
    const { win, doc } = subframe()
    runGuard(win)
    const f = doc.createElement('iframe')
    f.srcdoc = '<!doctype html><p>x</p>'
    const sd = f.getAttribute('srcdoc')!
    expect(sd.startsWith('<!doctype html><script data-k2-sealed-guard="">')).toBe(true)
    expect(sd).toContain('k2SealedFrameGuard')
    expect(sd.endsWith('<p>x</p>')).toBe(true)
    // Setting it again doesn't stack a second copy.
    f.setAttribute('srcdoc', sd)
    expect(f.getAttribute('srcdoc')).toBe(sd)
    doc.body.innerHTML = '<iframe srcdoc="<p>y</p>"></iframe>'
    await flush()
    expect(doc.querySelector('iframe')!.getAttribute('srcdoc')!.startsWith('<script data-k2-sealed-guard="">')).toBe(true)
  })

  it('the guard text it copies into a nested frame is a working guard', () => {
    const { win, doc } = subframe()
    runGuard(win)
    const f = doc.createElement('iframe')
    f.srcdoc = ''
    const sd = f.getAttribute('srcdoc')!
    const m = /^<script data-k2-sealed-guard="">([\s\S]*)<\/script>$/.exec(sd)
    expect(m).not.toBeNull()
    const child = subframe()
    withStandIns(child.win)
    new Function('window', m![1])(child.win)
    expect(typeof child.win.RTCPeerConnection).toBe('undefined')
  })

  it('patched prototypes stay patched (non-configurable)', () => {
    const { win } = subframe()
    runGuard(win)
    const El = (win as unknown as { Element: typeof Element }).Element
    expect(Object.getOwnPropertyDescriptor(El.prototype, 'setAttribute')?.configurable).toBe(false)
    expect(() =>
      Object.defineProperty(El.prototype, 'setAttribute', { value: function () {} }),
    ).toThrow(TypeError)
  })
})
