// @vitest-environment node
// The Desktop tab's pure modules (prd-remote-desktop-tab-v1 §6, §8, §9 R14),
// fail loud, with NO DOM: this file runs in the node environment and
// asserts `window` and `document` don't exist, so any DOM reach in the
// modules it imports would throw here (the phone lifts them unchanged).
//  - desktop-wire: the JSON goldens are the Rust goldens in
//    crates/k2-core/src/desktop/wire.rs, byte for byte; unknown `t` refused;
//  - desktop-keymap: the code list is exactly KEYS in keymap.rs; the
//    keyboard rules (Capture keys off keeps ⌘W for K2; on, the chords K2 or
//    the OS take first go to the keys menu; ⌃⌥⌘Esc leaves);
//  - desktop-input: caps, budget, coalescing, pointer mapping, flip, held;
//  - desktop-serialize and tabState.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  CLOSE_AUTH_REVOKED,
  CLOSE_DESKTOP_OFF,
  CLOSE_FORBIDDEN,
  VIDEO_CODEC_H264,
  VIDEO_CODEC_VP8,
  closeReasonText,
  isInputMsg,
  parseServerMsg,
  parseViewerMsg,
  serializeViewerMsg,
  type ServerMsg,
  type ViewerMsg,
} from './desktop-wire'
import {
  CANT_CAPTURE,
  DESKTOP_KEY_CODES,
  MODIFIERS,
  chordEvents,
  keysMenu,
  routeKey,
} from './desktop-keymap'
import {
  HeldInput,
  InputBudget,
  TEXT_MAX_CHARS,
  CLIPBOARD_MAX_BYTES,
  checkInput,
  coalesce,
  flipModifier,
  keyCodeToSend,
  mapPointer,
  splitText,
} from './desktop-input'
import { desktopDataFromSerializedBrowser, serializeDesktopItem } from './desktop-serialize'
import { cardFor, openDesktopIsForMe, STATE_CARDS, tabState, type DesktopStatus } from './desktop-state'

const REPO = join(__dirname, '../../../..')

describe('pure modules run with no DOM', () => {
  it('has no window or document here', () => {
    expect(typeof (globalThis as { window?: unknown }).window).toBe('undefined')
    expect(typeof (globalThis as { document?: unknown }).document).toBe('undefined')
  })
})

const DISPLAY = { id: '1', name: 'Built-in', widthPt: 1512, heightPt: 982, widthPx: 3024, heightPx: 1964, scale: 2.0, main: true }

describe('desktop-wire: the Rust goldens', () => {
  const server: Array<[ServerMsg, string]> = [
    [
      {
        t: 'hello', desktop: 'console', display: '1', displays: [DISPLAY], codec: 'h264', window: 3, fps: 60, path: 'lan',
        lane: 'main', encoder: 'hardware', mode: 'view', controller: null, os: 'macos', localWins: true, sameMachine: false,
      },
      '{"t":"hello","desktop":"console","display":"1","displays":[{"id":"1","name":"Built-in","widthPt":1512,"heightPt":982,"widthPx":3024,"heightPx":1964,"scale":2,"main":true}],"codec":"h264","window":3,"fps":60,"path":"lan","lane":"main","encoder":"hardware","mode":"view","controller":null,"os":"macos","localWins":true,"sameMachine":false}',
    ],
    [{ t: 'state', state: 'locked', message: 'This Mac is locked.' }, '{"t":"state","state":"locked","message":"This Mac is locked."}'],
    [{ t: 'control', controller: 'rosson', you: true, localHold: false }, '{"t":"control","controller":"rosson","you":true,"localHold":false}'],
    [{ t: 'viewers', viewers: [{ name: 'rosson', mode: 'control' }] }, '{"t":"viewers","viewers":[{"name":"rosson","mode":"control"}]}'],
    [{ t: 'cursor', id: 7, hotX: 1, hotY: 2, png: 'iVBO' }, '{"t":"cursor","id":7,"hotX":1,"hotY":2,"png":"iVBO"}'],
    [{ t: 'cursor-pos', x: 10.5, y: 20, visible: true }, '{"t":"cursor-pos","x":10.5,"y":20,"visible":true}'],
    [{ t: 'clipboard', text: 'hi' }, '{"t":"clipboard","text":"hi"}'],
    [{ t: 'clipboard', text: null, reason: 'concealed' }, '{"t":"clipboard","text":null,"reason":"concealed"}'],
    [{ t: 'fps', fps: 30 }, '{"t":"fps","fps":30}'],
    [{ t: 'codec', codec: 'vp8' }, '{"t":"codec","codec":"vp8"}'],
    [{ t: 'error', code: 'desktop_watch_only', message: "You're watching." }, '{"t":"error","code":"desktop_watch_only","message":"You\'re watching."}'],
  ]
  it.each(server)('server %#: parses the golden and keeps its fields', (msg, golden) => {
    expect(parseServerMsg(golden)).toEqual(msg)
    expect(JSON.stringify(msg)).toBe(golden)
  })

  const viewer: Array<[ViewerMsg, string]> = [
    [{ t: 'ack', seq: 9 }, '{"t":"ack","seq":9}'],
    [{ t: 'fps', fps: 0 }, '{"t":"fps","fps":0}'],
    [{ t: 'keyframe' }, '{"t":"keyframe"}'],
    [{ t: 'pointer', x: 1.5, y: 2 }, '{"t":"pointer","x":1.5,"y":2}'],
    [{ t: 'button', button: 'forward', down: true }, '{"t":"button","button":"forward","down":true}'],
    [{ t: 'wheel', dx: 0, dy: -3, mode: 'line' }, '{"t":"wheel","dx":0,"dy":-3,"mode":"line"}'],
    [{ t: 'key', code: 'KeyW', down: false }, '{"t":"key","code":"KeyW","down":false}'],
    [{ t: 'text', text: 'é' }, '{"t":"text","text":"é"}'],
    [{ t: 'clipboard-set', text: 'x' }, '{"t":"clipboard-set","text":"x"}'],
    [{ t: 'clipboard-get' }, '{"t":"clipboard-get"}'],
    [{ t: 'take-control' }, '{"t":"take-control"}'],
    [{ t: 'hand-back' }, '{"t":"hand-back"}'],
    [{ t: 'display', id: '2' }, '{"t":"display","id":"2"}'],
    [{ t: 'view', on: true }, '{"t":"view","on":true}'],
  ]
  it.each(viewer)('viewer %#: serializes to the golden and parses back', (msg, golden) => {
    expect(serializeViewerMsg(msg)).toBe(golden)
    expect(parseViewerMsg(golden)).toEqual(msg)
  })

  it('refuses unknown messages, as the daemon does', () => {
    expect(parseViewerMsg('{"t":"touch","x":1,"y":2}')).toBeNull()
    expect(parseViewerMsg('{"t":"button","button":"thumb","down":true}')).toBeNull()
    expect(parseServerMsg('{"t":"sim-hold"}')).toBeNull()
    expect(parseServerMsg('not json')).toBeNull()
    expect(parseServerMsg('[1]')).toBeNull()
  })

  it('input messages are the ones that act (Rust is_input)', () => {
    const acts: ViewerMsg[] = [
      { t: 'pointer', x: 0, y: 0 }, { t: 'button', button: 'left', down: true }, { t: 'wheel', dx: 0, dy: 1, mode: 'pixel' },
      { t: 'key', code: 'KeyA', down: true }, { t: 'text', text: 'a' }, { t: 'clipboard-set', text: 'a' }, { t: 'clipboard-get' },
    ]
    const watch: ViewerMsg[] = [
      { t: 'ack', seq: 0 }, { t: 'fps', fps: 1 }, { t: 'keyframe' }, { t: 'take-control' }, { t: 'hand-back' },
      { t: 'display', id: '1' }, { t: 'view', on: false },
    ]
    expect(acts.every(isInputMsg)).toBe(true)
    expect(watch.some(isInputMsg)).toBe(false)
  })

  it('close codes and codec bytes are the sims (and VP8 = 2)', () => {
    expect([CLOSE_AUTH_REVOKED, CLOSE_FORBIDDEN, CLOSE_DESKTOP_OFF]).toEqual([4401, 4403, 4410])
    expect([VIDEO_CODEC_H264, VIDEO_CODEC_VP8]).toEqual([1, 2])
    expect(closeReasonText(4403, 'kicked')).toBe('The owner disconnected you.')
    expect(closeReasonText(4410, 'desktop_off')).toBe('Remote desktop was turned off on this computer.')
    expect(closeReasonText(4401, 'grant_gone')).toBe('Your access to this screen was taken away.')
    expect(closeReasonText(1006, '')).toBe('The connection to this computer closed.')
  })
})

describe('desktop-keymap', () => {
  it('lists exactly the codes of crates/k2-core/src/desktop/keymap.rs KEYS', () => {
    const rs = readFileSync(join(REPO, 'crates/k2-core/src/desktop/keymap.rs'), 'utf8')
    const start = rs.indexOf('pub const KEYS')
    const end = rs.indexOf('];', start)
    expect(start).toBeGreaterThan(0)
    const rows = [...rs.slice(start, end).matchAll(/\("([A-Za-z0-9]+)",\s*(?:Some|None)/g)].map((m) => m[1])
    expect(rows.length).toBeGreaterThan(100)
    expect([...DESKTOP_KEY_CODES].sort()).toEqual([...rows].sort())
    expect(new Set(DESKTOP_KEY_CODES).size).toBe(DESKTOP_KEY_CODES.length)
    const mods = [...rs.matchAll(/pub const MODIFIERS: \[&str; 8\] = \[([^\]]+)\]/g)][0][1]
    expect([...mods.matchAll(/"([A-Za-z]+)"/g)].map((m) => m[1])).toEqual([...MODIFIERS])
  })

  it('Capture keys off: ⌘W, ⌘T and ⌘L stay K2’s on a Mac; plain keys go to the desktop', () => {
    expect(routeKey({ meta: true, code: 'KeyW' }, false, true)).toBe('k2')
    expect(routeKey({ meta: true, code: 'KeyT' }, false, true)).toBe('k2')
    expect(routeKey({ meta: true, code: 'KeyL' }, false, true)).toBe('k2')
    expect(routeKey({ code: 'KeyW' }, false, true)).toBe('send')
    expect(routeKey({ ctrl: true, code: 'KeyC' }, false, true)).toBe('send')
    // On a PC viewer K2's command key is Ctrl.
    expect(routeKey({ ctrl: true, code: 'KeyW' }, false, false)).toBe('k2')
    expect(routeKey({ code: 'NotAKey' }, false, true)).toBe('k2')
  })

  it('Capture keys on: everything goes, the chords K2 or the OS take first go to the keys menu, ⌃⌥⌘Esc leaves', () => {
    expect(routeKey({ meta: true, code: 'KeyW' }, true, true)).toBe('send')
    for (const e of CANT_CAPTURE) expect(routeKey(e.chord, true, true)).toBe('keys-menu')
    for (const id of ['cmd-q', 'cmd-tab', 'cmd-space', 'opt-cmd-esc']) expect(CANT_CAPTURE.some((e) => e.id === id)).toBe(true)
    expect(routeKey({ ctrl: true, alt: true, meta: true, code: 'Escape' }, true, true)).toBe('release')
    expect(keysMenu('macos').map((e) => e.id)).not.toContain('ctrl-alt-del')
    expect(keysMenu('linux').map((e) => e.id)).toContain('ctrl-alt-del')
    expect(chordEvents({ meta: true, shift: true, code: 'KeyF' })).toEqual([
      { code: 'ShiftLeft', down: true }, { code: 'MetaLeft', down: true }, { code: 'KeyF', down: true },
      { code: 'KeyF', down: false }, { code: 'MetaLeft', down: false }, { code: 'ShiftLeft', down: false },
    ])
  })
})

describe('desktop-input', () => {
  it('coalescing keeps buttons, keys and text (the Rust golden)', () => {
    const batch: ViewerMsg[] = [
      { t: 'pointer', x: 1, y: 1 }, { t: 'pointer', x: 2, y: 2 }, { t: 'button', button: 'left', down: true },
      { t: 'pointer', x: 3, y: 3 }, { t: 'pointer', x: 4, y: 4 }, { t: 'key', code: 'KeyA', down: true },
      { t: 'key', code: 'KeyA', down: true }, { t: 'text', text: 'a' }, { t: 'text', text: 'b' },
      { t: 'wheel', dx: 0, dy: 1, mode: 'pixel' }, { t: 'wheel', dx: 1, dy: 2, mode: 'pixel' }, { t: 'wheel', dx: 0, dy: 1, mode: 'line' },
      { t: 'button', button: 'left', down: false }, { t: 'pointer', x: 5, y: 5 },
    ]
    expect(coalesce(batch)).toEqual([
      { t: 'pointer', x: 2, y: 2 }, { t: 'button', button: 'left', down: true }, { t: 'pointer', x: 4, y: 4 },
      { t: 'key', code: 'KeyA', down: true }, { t: 'key', code: 'KeyA', down: true }, { t: 'text', text: 'a' },
      { t: 'text', text: 'b' }, { t: 'wheel', dx: 1, dy: 3, mode: 'pixel' }, { t: 'wheel', dx: 0, dy: 1, mode: 'line' },
      { t: 'button', button: 'left', down: false }, { t: 'pointer', x: 5, y: 5 },
    ])
  })

  it('text rules are the sim’s; long text splits', () => {
    expect(checkInput({ t: 'text', text: 'héllo' })).toBeNull()
    for (const bad of ['', 'ls\n', 'a\tb', '\u001b[A']) expect(checkInput({ t: 'text', text: bad })?.code).toBe('desktop_invalid')
    expect(checkInput({ t: 'text', text: 'x'.repeat(TEXT_MAX_CHARS + 1) })).not.toBeNull()
    expect(checkInput({ t: 'text', text: 'x'.repeat(TEXT_MAX_CHARS) })).toBeNull()
    expect(checkInput({ t: 'clipboard-set', text: 'line\nline\t' })).toBeNull()
    expect(checkInput({ t: 'clipboard-set', text: 'x'.repeat(CLIPBOARD_MAX_BYTES + 1) })).not.toBeNull()
    expect(checkInput({ t: 'clipboard-set', text: '\u0007' })).not.toBeNull()
    expect(checkInput({ t: 'key', code: 'NotAKey', down: true })).not.toBeNull()
    expect(checkInput({ t: 'pointer', x: Number.NaN, y: 1 })).not.toBeNull()
    const parts = splitText('😀'.repeat(300))
    expect(parts.map((p) => Array.from(p).length)).toEqual([256, 44])
    expect(parts.join('')).toBe('😀'.repeat(300))
  })

  it('the budget limits each kind and never drops a release', () => {
    const t0 = 1_000
    const b = new InputBudget(t0)
    for (let i = 0; i < 4; i++) expect(b.take({ t: 'text', text: 'a' }, t0)).toBeNull()
    expect(b.take({ t: 'text', text: 'a' }, t0)?.code).toBe('desktop_rate_limited')
    expect(b.take({ t: 'text', text: 'a' }, t0 + 300)).toBeNull()
    expect(b.take({ t: 'clipboard-set', text: 'a' }, t0)).toBeNull()
    expect(b.take({ t: 'clipboard-set', text: 'a' }, t0)).not.toBeNull()
    for (let i = 0; i < 120; i++) expect(b.take({ t: 'key', code: 'KeyA', down: true }, t0)).toBeNull()
    expect(b.take({ t: 'key', code: 'KeyA', down: true }, t0)).not.toBeNull()
    for (let i = 0; i < 500; i++) {
      expect(b.take({ t: 'key', code: 'KeyA', down: false }, t0)).toBeNull()
      expect(b.take({ t: 'button', button: 'left', down: false }, t0)).toBeNull()
    }
    for (let i = 0; i < 240; i++) expect(b.take({ t: 'pointer', x: 1, y: 1 }, t0)).toBeNull()
    expect(b.take({ t: 'pointer', x: 1, y: 1 }, t0)).not.toBeNull()
  })

  it('maps the canvas to display points at fit (letterboxed) and 1:1 (scrolled)', () => {
    // 1600x1000 display in a 800x800 canvas: scale 0.5, 150 px bands top and bottom.
    const base = { canvasW: 800, canvasH: 800, widthPt: 1600, heightPt: 1000, fit: 'fit' as const }
    expect(mapPointer({ ...base, cx: 400, cy: 400 })).toEqual({ x: 800, y: 500 })
    expect(mapPointer({ ...base, cx: 0, cy: 150 })).toEqual({ x: 0, y: 0 })
    expect(mapPointer({ ...base, cx: 400, cy: 100 })).toBeNull()
    const one = { canvasW: 800, canvasH: 600, widthPt: 1600, heightPt: 1000, fit: '1:1' as const, cssPerPt: 1 }
    expect(mapPointer({ ...one, cx: 10, cy: 20, scrollX: 100, scrollY: 50 })).toEqual({ x: 110, y: 70 })
    expect(mapPointer({ ...one, cx: 10, cy: 20, scrollX: 100, scrollY: 50, cssPerPt: 2 })).toEqual({ x: 55, y: 35 })
    expect(mapPointer({ ...one, cx: 1700, cy: 0 })).toBeNull()
  })

  it('flips ⌘ and Ctrl only when the switch is on and the OS families differ', () => {
    expect(flipModifier('MetaLeft')).toBe('ControlLeft')
    expect(flipModifier('ControlRight')).toBe('MetaRight')
    expect(flipModifier('AltLeft')).toBe('AltLeft')
    expect(keyCodeToSend('MetaLeft', true, false, true)).toBe('ControlLeft')
    expect(keyCodeToSend('MetaLeft', true, true, true)).toBe('MetaLeft')
    expect(keyCodeToSend('MetaLeft', true, false, false)).toBe('MetaLeft')
  })

  it('held keys release newest first, once (R3)', () => {
    const h = new HeldInput()
    h.note({ t: 'key', code: 'MetaLeft', down: true })
    h.note({ t: 'button', button: 'left', down: true })
    h.note({ t: 'key', code: 'ShiftLeft', down: true })
    h.note({ t: 'key', code: 'ShiftLeft', down: false })
    h.note({ t: 'key', code: 'KeyC', down: true })
    h.note({ t: 'key', code: 'KeyC', down: true })
    h.note({ t: 'pointer', x: 1, y: 1 })
    expect(h.releaseAll()).toEqual([
      { t: 'key', code: 'KeyC', down: false }, { t: 'button', button: 'left', down: false }, { t: 'key', code: 'MetaLeft', down: false },
    ])
    expect(h.isEmpty()).toBe(true)
    expect(h.releaseAll()).toEqual([])
  })
})

describe('desktop-serialize', () => {
  it('round-trips the marker and refuses what isn’t one', () => {
    const s = serializeDesktopItem('i1', { desktopId: 'console', displayId: '2', fit: '1:1' })
    expect(s).toEqual({ id: 'i1', type: 'browser', url: '', title: 'Desktop', desktop: { v: 1, desktopId: 'console', displayId: '2', fit: '1:1' } })
    expect(desktopDataFromSerializedBrowser(JSON.parse(JSON.stringify(s)))).toEqual({ desktopId: 'console', displayId: '2', fit: '1:1' })
    expect(desktopDataFromSerializedBrowser({ id: 'x', type: 'browser', url: '' })).toBeNull()
    expect(desktopDataFromSerializedBrowser({ id: 'x', type: 'browser', desktop: {} })).toBeNull()
    expect(desktopDataFromSerializedBrowser({ id: 'x', type: 'terminal', desktop: { v: 1 } })).toBeNull()
    expect(desktopDataFromSerializedBrowser({ id: 'x', type: 'browser', desktop: { v: 1, fit: 'zoom' } })).toEqual({ desktopId: 'console' })
  })
})

function status(over: Omit<Partial<DesktopStatus>, 'you'> & { you?: Partial<DesktopStatus['you']> } = {}): DesktopStatus {
  const { you, ...rest } = over
  return {
    desktop: 'console', machine: 'mini', enabled: true, backend: 'macos', state: 'ready', message: '', lane: 'main',
    you: { role: 'owner', mayView: true, mayControl: true, mayManage: true, sameMachine: false, ...you },
    ...rest,
  }
}

describe('tabState', () => {
  it('off, then this machine, then access, then the machine', () => {
    expect(tabState(status({ enabled: false, you: { sameMachine: true, mayView: false } }))).toBe('off')
    expect(tabState(status({ you: { sameMachine: true } }))).toBe('same_machine')
    expect(tabState(status({ you: { mayView: false, role: 'member' } }))).toBe('not_allowed')
    expect(tabState(status({ state: 'locked' }))).toBe('locked')
    expect(tabState(status({ state: 'blocked_in_login_items' }))).toBe('blocked_in_login_items')
    expect(tabState(status())).toBe('ready')
    // A state this client doesn't know is never `ready`.
    expect(tabState(status({ state: 'warp_drive' }))).toBe('unavailable')
  })

  it('every card has its line and at most one action; owner-only actions are dropped for others', () => {
    for (const [state, spec] of Object.entries(STATE_CARDS)) expect(spec.message.length, state).toBeGreaterThan(0)
    expect(cardFor('off', true).action?.id).toBe('turn-on')
    expect(cardFor('off', false).action).toBeUndefined()
    expect(cardFor('needs_permission', false).action).toBeUndefined()
    expect(cardFor('denied', false).action?.id).toBe('try-again')
    expect(cardFor('needs_update', false).action?.id).toBe('retry')
    expect(cardFor('unavailable', true).action).toBeUndefined()
  })

  it('open_desktop names this client by its identity on that server', () => {
    expect(openDesktopIsForMe('owner', false, undefined)).toBe(true)
    expect(openDesktopIsForMe('rosson', false, undefined)).toBe(false)
    expect(openDesktopIsForMe('rosson', true, 'rosson')).toBe(true)
    expect(openDesktopIsForMe('owner', true, 'rosson')).toBe(false)
    expect(openDesktopIsForMe('', true, '')).toBe(false)
    // The owner asked from a K2 shell tab: Owner logins watching it open too.
    expect(openDesktopIsForMe('owner', true, 'rosson', ['rosson'])).toBe(true)
    expect(openDesktopIsForMe('owner', true, 'mel', ['rosson'])).toBe(false)
    expect(openDesktopIsForMe('ann', true, 'rosson', ['rosson'])).toBe(false)
  })
})
