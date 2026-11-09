// @vitest-environment jsdom
// Thread view's terminal reveal (prd-thread-view-focus-v1 §6): placement at
// the 900px threshold, sizing, motion, memory of neither side remounting.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render, screen } from '@testing-library/react'
import { useEffect, useState } from 'react'

import { SplitSessionColumns } from './SplitSessionColumns'
import {
  REVEAL_PLACEMENT_SETTLE_MS,
  parseCssDurationMs,
  registerRevealTarget,
  revealPlacementNext,
  revealTargetsForTests,
  revealTerminalSizePx,
  toggleRevealForFocus,
  useTerminalRevealPresence,
} from './threadReveal'

describe('revealPlacementNext', () => {
  it('opens on the left from 900px wide, at the bottom below it', () => {
    expect(revealPlacementNext(900, null)).toBe('left')
    expect(revealPlacementNext(899, null)).toBe('bottom')
    expect(revealPlacementNext(1400, null)).toBe('left')
  })
  it('going narrow flips to the bottom under 900', () => {
    expect(revealPlacementNext(899, 'left')).toBe('bottom')
    expect(revealPlacementNext(900, 'left')).toBe('left')
  })
  it('going wide returns to the left only at 924', () => {
    expect(revealPlacementNext(923, 'bottom')).toBe('bottom')
    expect(revealPlacementNext(924, 'bottom')).toBe('left')
  })
  it('a 0 width keeps the placement', () => {
    expect(revealPlacementNext(0, 'bottom')).toBe('bottom')
    expect(revealPlacementNext(0, null)).toBe('left')
  })
})

describe('revealTerminalSizePx', () => {
  it('left: 45% of the pane, between 420 and 640', () => {
    expect(revealTerminalSizePx('left', 900, 800)).toBe(420) // 405 → floor
    expect(revealTerminalSizePx('left', 1200, 800)).toBe(540)
    expect(revealTerminalSizePx('left', 2000, 800)).toBe(640)
  })
  it('bottom: 45%, at least 180, leaving 240 for the Thread where the pane allows', () => {
    expect(revealTerminalSizePx('bottom', 800, 1000)).toBe(450)
    expect(revealTerminalSizePx('bottom', 800, 500)).toBe(225)
    // 45% of 420 = 189, but 420 - 240 = 180 is what leaves the Thread room.
    expect(revealTerminalSizePx('bottom', 800, 420)).toBe(180)
    expect(revealTerminalSizePx('bottom', 800, 300)).toBe(180)
  })
})

describe('parseCssDurationMs', () => {
  it('reads ms and s, null otherwise', () => {
    expect(parseCssDurationMs('150ms')).toBe(150)
    expect(parseCssDurationMs(' 0.22s ')).toBe(220)
    expect(parseCssDurationMs('fast')).toBeNull()
    expect(parseCssDurationMs('')).toBeNull()
  })
})

// ── The columns ────────────────────────────────────────────────────────

let pane = { width: 1200, height: 800 }
let roFires: Array<() => void> = []
let animations: FakeAnimation[] = []

class FakeAnimation {
  onfinish: (() => void) | null = null
  canceled = false
  reversed = 0
  constructor(
    public keyframes: Keyframe[],
    public options: KeyframeAnimationOptions,
  ) {
    animations.push(this)
  }
  cancel(): void {
    this.canceled = true
  }
  reverse(): void {
    this.reversed += 1
  }
  finish(): void {
    this.onfinish?.()
  }
}

function setMotion(on: boolean): void {
  vi.stubGlobal('matchMedia', (q: string) => ({
    matches: q.includes('reduce') ? !on : false,
    addEventListener: () => {},
    removeEventListener: () => {},
  }))
  // jsdom's window is the global window here.
  window.matchMedia = globalThis.matchMedia
}

const mounts = { terminal: 0, thread: 0 }

function PTY(): React.JSX.Element {
  useEffect(() => {
    mounts.terminal += 1
  }, [])
  return <textarea data-testid="pty" />
}
function Thread(): React.JSX.Element {
  useEffect(() => {
    mounts.thread += 1
  }, [])
  return (
    <div data-testid="thread">
      <div data-testid="agent-session-thread-compose-slot">
        <div data-compose-bar="">
          <textarea data-testid="composer" />
        </div>
      </div>
    </div>
  )
}

let setOpen: (next: boolean) => void = () => {}
let setKey: (next: string) => void = () => {}

function Frame({ startOpen = false }: { startOpen?: boolean }): React.JSX.Element {
  const [open, setO] = useState(startOpen)
  const [motionKey, setK] = useState('conv-1')
  setOpen = setO
  setKey = setK
  const presence = useTerminalRevealPresence(open, true)
  return (
    <SplitSessionColumns
      showSplit={false}
      hidePty={true}
      left="terminal"
      right="thread"
      terminal={<PTY />}
      sharedTerminal={<div />}
      renderView={() => <div />}
      single={<Thread />}
      reveal={{ open, shown: presence.shown, motionKey, onClosed: presence.onClosed }}
    />
  )
}

const columns = (): HTMLElement => screen.getByTestId('session-split-columns')
const terminalColumn = (): HTMLElement => screen.getByTestId('session-reveal-terminal')

beforeEach(() => {
  pane = { width: 1200, height: 800 }
  roFires = []
  animations = []
  mounts.terminal = 0
  mounts.thread = 0
  setMotion(true)
  vi.stubGlobal(
    'ResizeObserver',
    class {
      cb: () => void
      constructor(cb: () => void) {
        this.cb = cb
      }
      observe() {
        roFires.push(this.cb)
      }
      disconnect() {
        roFires = roFires.filter((f) => f !== this.cb)
      }
      unobserve() {}
    },
  )
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(() => new DOMRect(0, 0, pane.width, pane.height))
  HTMLElement.prototype.animate = function animate(frames: Keyframe[], options: KeyframeAnimationOptions) {
    return new FakeAnimation(frames, options) as unknown as Animation
  } as typeof HTMLElement.prototype.animate
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  vi.useRealTimers()
  delete (HTMLElement.prototype as unknown as { animate?: unknown }).animate
})

function fireResize(): void {
  act(() => {
    for (const fire of roFires.slice()) fire()
  })
}

describe('revealing the terminal', () => {
  it('starts hidden (display:none) with the Thread alone', () => {
    render(<Frame />)
    expect(terminalColumn().style.display).toBe('none')
    expect(screen.getByTestId('pty')).toBeTruthy()
    expect(columns().getAttribute('data-reveal-placement')).toBeNull()
  })

  it('reveals on the left at 900 wide or more, at 45% of the pane', () => {
    render(<Frame />)
    act(() => setOpen(true))
    expect(terminalColumn().style.display).toBe('flex')
    expect(columns().getAttribute('data-reveal-placement')).toBe('left')
    expect(terminalColumn().style.width).toBe('540px')
    expect(columns().style.flexDirection).toBe('row')
  })

  it('reveals from the bottom below 900, the Thread above it', () => {
    pane = { width: 899, height: 800 }
    render(<Frame />)
    act(() => setOpen(true))
    expect(columns().getAttribute('data-reveal-placement')).toBe('bottom')
    expect(terminalColumn().style.height).toBe('360px')
    // The terminal stays the first child; column-reverse draws it below.
    expect(columns().style.flexDirection).toBe('column-reverse')
    expect(columns().firstElementChild).toBe(terminalColumn())
  })

  it('never remounts the terminal or the Thread across toggles', () => {
    render(<Frame />)
    const ptyNode = screen.getByTestId('pty')
    const threadNode = screen.getByTestId('thread')
    for (let i = 0; i < 3; i += 1) {
      act(() => setOpen(true))
      act(() => animations.at(-1)?.finish())
      act(() => setOpen(false))
      act(() => animations.at(-2)?.finish())
    }
    expect(screen.getByTestId('pty')).toBe(ptyNode)
    expect(screen.getByTestId('thread')).toBe(threadNode)
    expect(mounts.terminal).toBe(1)
    expect(mounts.thread).toBe(1)
  })

  it('moves only transform, in the Style tokens timing, offset by half the terminal on the left', () => {
    vi.spyOn(window, 'getComputedStyle').mockReturnValue({
      getPropertyValue: (name: string) =>
        name === '--motion-duration-medium' ? '220ms' : name === '--motion-ease-standard' ? 'ease-out' : '',
    } as unknown as CSSStyleDeclaration)
    render(<Frame />)
    act(() => setOpen(true))
    expect(animations).toHaveLength(2)
    expect(animations.every((a) => a.options.duration === 220 && a.options.easing === 'ease-out')).toBe(true)
    expect(animations[0].keyframes[0].transform).toBe('translateX(-100%)')
    expect(animations[1].keyframes[0].transform).toBe('translateX(-270px)')
    for (const a of animations) {
      expect(a.keyframes.every((f) => Object.keys(f).join() === 'transform')).toBe(true)
    }
  })

  it('bottom: the Thread starts offset by the terminal height', () => {
    pane = { width: 800, height: 800 }
    render(<Frame />)
    act(() => setOpen(true))
    expect(animations[0].keyframes[0].transform).toBe('translateY(100%)')
    expect(animations[1].keyframes[0].transform).toBe('translateY(360px)')
  })

  it('closing keeps the column up until the slide finishes, then hides it', () => {
    render(<Frame />)
    act(() => setOpen(true))
    act(() => animations[0].finish())
    expect(animations.every((a) => a.canceled)).toBe(true)
    animations = []
    act(() => setOpen(false))
    expect(terminalColumn().style.display).toBe('flex')
    expect(animations[0].reversed).toBe(1)
    act(() => animations[0].finish())
    expect(terminalColumn().style.display).toBe('none')
    expect(animations.every((a) => a.canceled)).toBe(true)
  })

  it('toggling mid-slide reverses from where it is', () => {
    render(<Frame />)
    act(() => setOpen(true))
    expect(animations).toHaveLength(2)
    act(() => setOpen(false))
    expect(animations).toHaveLength(2)
    expect(animations[0].reversed).toBe(1)
    expect(animations[1].reversed).toBe(1)
    act(() => setOpen(true))
    expect(animations[0].reversed).toBe(2)
    // Ends open: nothing hidden, the slide let go.
    act(() => animations[0].finish())
    expect(terminalColumn().style.display).toBe('flex')
    expect(animations.every((a) => a.canceled)).toBe(true)
  })

  it('does not animate under reduced motion, and closes at once', () => {
    setMotion(false)
    render(<Frame />)
    act(() => setOpen(true))
    expect(animations).toHaveLength(0)
    expect(terminalColumn().style.display).toBe('flex')
    act(() => setOpen(false))
    expect(animations).toHaveLength(0)
    expect(terminalColumn().style.display).toBe('none')
  })

  it('does not animate on restore (opening already revealed)', () => {
    render(<Frame startOpen />)
    expect(animations).toHaveLength(0)
    expect(terminalColumn().style.display).toBe('flex')
  })

  it('does not animate moving to another conversation', () => {
    render(<Frame />)
    act(() => {
      setKey('conv-2')
      setOpen(true)
    })
    expect(animations).toHaveLength(0)
    expect(terminalColumn().style.display).toBe('flex')
  })

  it('re-checks placement only after the size settles, without animating', () => {
    vi.useFakeTimers()
    render(<Frame />)
    act(() => setOpen(true))
    act(() => animations[0].finish())
    animations = []
    pane = { width: 899, height: 800 }
    fireResize()
    act(() => vi.advanceTimersByTime(REVEAL_PLACEMENT_SETTLE_MS - 1))
    expect(columns().getAttribute('data-reveal-placement')).toBe('left')
    // The width still follows the pane while it waits.
    expect(terminalColumn().style.width).toBe('420px')
    act(() => vi.advanceTimersByTime(1))
    expect(columns().getAttribute('data-reveal-placement')).toBe('bottom')
    expect(animations).toHaveLength(0)
    // Back to 923: stays below. 924: returns.
    pane = { width: 923, height: 800 }
    fireResize()
    act(() => vi.advanceTimersByTime(REVEAL_PLACEMENT_SETTLE_MS))
    expect(columns().getAttribute('data-reveal-placement')).toBe('bottom')
    pane = { width: 924, height: 800 }
    fireResize()
    act(() => vi.advanceTimersByTime(REVEAL_PLACEMENT_SETTLE_MS))
    expect(columns().getAttribute('data-reveal-placement')).toBe('left')
  })

  it('hiding while focus is in the terminal moves it to the Thread message box', () => {
    render(<Frame />)
    act(() => setOpen(true))
    act(() => animations.at(-1)?.finish())
    const pty = screen.getByTestId('pty') as HTMLTextAreaElement
    pty.focus()
    expect(document.activeElement).toBe(pty)
    act(() => setOpen(false))
    expect(document.activeElement).toBe(screen.getByTestId('composer'))
  })

  it('hiding with focus elsewhere leaves focus alone', () => {
    render(
      <>
        <input data-testid="elsewhere" />
        <Frame />
      </>,
    )
    act(() => setOpen(true))
    act(() => animations.at(-1)?.finish())
    const other = screen.getByTestId('elsewhere')
    other.focus()
    act(() => setOpen(false))
    expect(document.activeElement).toBe(other)
  })

  it('revealing leaves focus where it is', () => {
    render(
      <>
        <input data-testid="elsewhere" />
        <Frame />
      </>,
    )
    const other = screen.getByTestId('elsewhere')
    other.focus()
    act(() => setOpen(true))
    expect(document.activeElement).toBe(other)
  })
})

describe('Terminal and Split stay as they were', () => {
  it('Terminal view has no reveal box and a contents-only Thread slot', () => {
    render(
      <SplitSessionColumns
        showSplit={false}
        hidePty={false}
        left="terminal"
        right="thread"
        terminal={<PTY />}
        sharedTerminal={<div />}
        renderView={() => <div />}
        single={null}
      />,
    )
    expect(screen.getByTestId('session-single-slot').style.display).toBe('contents')
    expect(columns().style.flexDirection).toBe('column')
    expect(columns().style.overflow).toBe('')
    expect(screen.queryByTestId('session-reveal-terminal')).toBeNull()
  })

  it('Split renders no reveal and no Thread slot', () => {
    render(
      <SplitSessionColumns
        showSplit
        hidePty={false}
        left="terminal"
        right="thread"
        terminal={<PTY />}
        sharedTerminal={<div />}
        renderView={() => <Thread />}
        single={<div />}
      />,
    )
    expect(screen.queryByTestId('session-single-slot')).toBeNull()
    expect(screen.queryByTestId('session-reveal-terminal')).toBeNull()
  })
})

describe('toggleRevealForFocus', () => {
  const room = {}
  let toggled: string[] = []

  function target(name: string, group: number, visible = true): () => void {
    const column = document.createElement('div')
    column.setAttribute('data-tab-group-index', String(group))
    const paneEl = document.createElement('div')
    paneEl.dataset.pane = name
    const header = document.createElement('div')
    header.setAttribute('data-session-header', '')
    const button = document.createElement('button')
    button.getClientRects = (() => (visible ? [{}] : [])) as unknown as typeof button.getClientRects
    header.appendChild(button)
    paneEl.appendChild(header)
    const input = document.createElement('textarea')
    input.dataset.name = name
    paneEl.appendChild(input)
    column.appendChild(paneEl)
    document.body.appendChild(column)
    return registerRevealTarget({ room, element: button, enabled: () => true, toggle: () => toggled.push(name) })
  }

  beforeEach(() => {
    toggled = []
    document.body.innerHTML = ''
    revealTargetsForTests().clear()
  })

  it('prefers the pane holding focus', () => {
    target('a', 0)
    target('b', 1)
    ;(document.querySelector('[data-name="b"]') as HTMLElement).focus()
    expect(toggleRevealForFocus(room, 0)).toBe(true)
    expect(toggled).toEqual(['b'])
  })

  it('else acts on the active column', () => {
    target('a', 0)
    target('b', 1)
    expect(toggleRevealForFocus(room, 1)).toBe(true)
    expect(toggled).toEqual(['b'])
  })

  it('ignores hidden panes, other rooms and disabled toggles, and reports nothing to do', () => {
    target('hidden', 0, false)
    registerRevealTarget({
      room: {},
      element: document.body,
      enabled: () => true,
      toggle: () => toggled.push('other-room'),
    })
    expect(toggleRevealForFocus(room, 0)).toBe(false)
    expect(toggled).toEqual([])
  })

  it('does nothing when only another column shows Thread view', () => {
    target('a', 1)
    expect(toggleRevealForFocus(room, 0)).toBe(false)
    expect(toggled).toEqual([])
  })
})
