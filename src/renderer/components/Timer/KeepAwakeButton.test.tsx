// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { keepAwakeEntryFor, resetKeepAwakeForTests } from '@/stores/keep-awake'
import { primaryScope } from '@/kessel/server-scope'
import { KEEP_AWAKE_MODES, keepAwakeTone, parseKeepAwakeBody, type KeepAwakeStatus } from '@/lib/keep-awake'
import { SQUARE_CHECK_CLASS, SQUARE_RADIO_CLASS } from '@/components/ui'
import { TOP_BAR_ICON_STROKE_WIDTH } from './topBarIcon'

const h = vi.hoisted(() => ({
  remote: false,
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: (sel: (s: { activeHost: 'local' | { id: string } }) => unknown) =>
    sel({ activeHost: h.remote ? { id: 'remote-box' } : 'local' }),
  // The stores drop the window server's entry on a top-switcher change.
  onActiveHostChange: () => () => {},
}))

// The real TimerButton, idle, so its clock glyph can be compared to the mug.
vi.mock('@/stores/timer', () => {
  const idle = {
    status: 'idle',
    visible: true,
    pausedElapsed: 0,
    resumeTime: null,
    startTimer: () => {},
    pauseTimer: () => {},
    resumeTimer: () => {},
    stopTimer: () => {},
    showMemoDialog: false,
  }
  return {
    useTimerStore: (sel: (s: typeof idle) => unknown) => sel(idle),
    getElapsedMs: () => 0,
    formatElapsed: () => '0:00',
  }
})

import KeepAwakeButton from './KeepAwakeButton'
import TimerButton from './TimerButton'

function status(over: Partial<KeepAwakeStatus>): KeepAwakeStatus {
  return {
    mode: 'always',
    state: 'lid_closed_ok',
    label: 'Awake, lid closed OK (on power)',
    detail: 'Stays awake with the lid closed while on power.',
    held: true,
    lidHeld: true,
    workingSessions: 0,
    powerSource: { onAc: true, batteryPercent: 90 },
    batteryFloorPercent: 20,
    alsoOnBattery: false,
    lidClosed: true,
    lidSwitch: true,
    lidSetup: 'ready',
    lidSetupDetail: '',
    canSetUp: false,
    canApproveLid: false,
    canChange: true,
    lidDialogDeclined: false,
    platform: 'macos',
    ...over,
  }
}

const NOT_SET_UP = 'Lid closed will still sleep: the power helper is not set up on this Mac'
const SET_UP_HINT = 'Set up installs a small helper. macOS asks once for an admin password.'
const ASK_AN_ADMIN = 'An admin sets up the power helper, in K2 on the host Mac.'

/** One fixture per daemon state, with the daemon's own words. */
const STATES: { name: string; s: KeepAwakeStatus; tone: string }[] = [
  {
    name: 'off',
    s: status({ mode: 'off', state: 'off', label: 'Keep awake: off', detail: 'This computer sleeps as usual.', held: false, lidHeld: false }),
    tone: 'off',
  },
  {
    name: 'waiting',
    s: status({
      mode: 'working',
      state: 'waiting',
      label: 'Keep awake: waiting for agents',
      detail: 'Not holding. It holds while any agent is working, and for 1 minute after.',
      held: false,
      lidHeld: false,
    }),
    tone: 'armed',
  },
  { name: 'lid closed on power', s: status({}), tone: 'held' },
  {
    name: 'lid closed on battery',
    s: status({
      label: 'Awake, lid closed OK (on battery)',
      detail: 'Stays awake with the lid closed until the battery reaches 20%. Closed in a bag, a laptop can get hot.',
      powerSource: { onAc: false, batteryPercent: 64 },
      alsoOnBattery: true,
    }),
    tone: 'held',
  },
  {
    name: 'lid open only (not set up, local admin)',
    s: status({
      state: 'lid_open_only',
      label: 'Awake (lid open only)',
      detail: NOT_SET_UP,
      lidHeld: false,
      lidSetup: 'needs_setup',
      lidSetupDetail: SET_UP_HINT,
      canSetUp: true,
      canApproveLid: true,
    }),
    tone: 'held',
  },
  {
    name: 'lid open only (not set up, member or remote)',
    s: status({
      state: 'lid_open_only',
      label: 'Awake (lid open only)',
      detail: NOT_SET_UP,
      lidHeld: false,
      lidSetup: 'needs_setup',
      lidSetupDetail: ASK_AN_ADMIN,
    }),
    tone: 'held',
  },
  {
    name: 'lid open only (switch off)',
    s: status({
      state: 'lid_open_only',
      label: 'Awake (lid open only)',
      detail: 'Lid closed will still sleep: "Also with the lid closed" is off',
      lidHeld: false,
      lidClosed: false,
      lidSetup: 'needs_setup',
      lidSetupDetail: SET_UP_HINT,
      canSetUp: true,
      canApproveLid: true,
    }),
    tone: 'held',
  },
  {
    name: 'paused',
    s: status({
      state: 'paused',
      label: 'Paused: battery below 20%',
      detail: 'On battery below 20%, K2 lets this computer sleep.',
      held: false,
      lidHeld: false,
      powerSource: { onAc: false, batteryPercent: 12 },
    }),
    tone: 'warn',
  },
  {
    name: 'limited (Linux, no session)',
    s: status({
      state: 'limited',
      label: 'Keep awake: limited (no session)',
      detail: 'No login session on this machine, so the system refused the sleep lock.',
      held: false,
      lidHeld: false,
      platform: 'linux',
    }),
    tone: 'warn',
  },
  {
    name: 'Windows lid action',
    s: status({
      state: 'lid_open_only',
      label: 'Awake (lid open only)',
      detail:
        "Lid closed will still sleep: the power plan's lid action is Sleep. Control Panel → Hardware and Sound → Power Options → Choose what closing the lid does → When I close the lid → set \"Plugged in\" (and \"On battery\" if you want) to Do nothing → Save changes.",
      lidHeld: false,
      lidSwitch: false,
      lidClosed: false,
      platform: 'windows',
    }),
    tone: 'held',
  },
  {
    name: 'error',
    s: status({
      state: 'error',
      label: 'Keep awake: not held',
      detail: 'The system refused: IOPMAssertionCreateWithName returned 0xe00002c1',
      held: false,
      lidHeld: false,
    }),
    tone: 'warn',
  },
]

beforeEach(() => {
  cleanup()
  h.remote = false
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  resetKeepAwakeForTests()
})

async function renderWith(s: KeepAwakeStatus): Promise<HTMLElement> {
  h.daemonCliGet.mockResolvedValue({ keepAwake: s })
  render(<KeepAwakeButton />)
  const button = await screen.findByTestId('keep-awake')
  return button
}

describe('KeepAwakeButton', () => {
  it('asks the window server for power/status', async () => {
    await renderWith(STATES[0].s)
    expect(h.daemonCliGet).toHaveBeenCalledWith('power/status')
  })

  for (const { name, s, tone } of STATES) {
    it(`renders the daemon's honest state: ${name}`, async () => {
      const button = await renderWith(s)
      expect(button.getAttribute('aria-label')).toBe(`Keep awake: ${s.label}`)
      expect(button.getAttribute('data-state')).toBe(s.state)
      expect(button.getAttribute('data-tone')).toBe(tone)
      expect(keepAwakeTone(s)).toBe(tone)

      fireEvent.click(button)
      const menu = await screen.findByTestId('keep-awake-menu')
      expect(screen.getByTestId('keep-awake-label').textContent).toBe(s.label)
      expect(screen.getByTestId('keep-awake-detail').textContent).toBe(s.detail)
      const checked = Array.from(menu.querySelectorAll<HTMLInputElement>('input[type="radio"]')).filter((el) => el.checked)
      expect(checked.map((el) => el.getAttribute('data-testid'))).toEqual([`keep-awake-mode-${s.mode}`])
      const lid = screen.queryByTestId('keep-awake-lid') as HTMLInputElement | null
      expect(lid !== null).toBe(s.lidSwitch)
      if (lid) expect(lid.checked).toBe(s.lidClosed)
      const needsSetup = s.lidClosed && s.lidSetup === 'needs_setup'
      expect(screen.queryByTestId('keep-awake-setup') !== null).toBe(needsSetup && s.canSetUp)
      if (needsSetup) {
        const note = screen.getByTestId('keep-awake-lid-note').textContent ?? ''
        expect(note).toContain('lid closed will still sleep')
        expect(note).toContain(s.lidSetupDetail)
      }
      // The old dialog button and its words are gone from the menu.
      expect(screen.queryByTestId('keep-awake-approve')).toBeNull()
      expect(menu.textContent).not.toMatch(/admin dialog/i)
      expect(screen.queryByTestId('keep-awake-battery') !== null).toBe(s.platform === 'macos')
    })
  }

  it('uses the square radio and checkbox, not native rounded controls', async () => {
    const button = await renderWith(STATES[2].s)
    fireEvent.click(button)
    const menu = await screen.findByTestId('keep-awake-menu')
    expect(menu.getAttribute('role')).toBe('dialog')
    const group = screen.getByRole('radiogroup', { name: 'Keep awake mode' })
    const radios = screen.getAllByRole('radio')
    expect(radios.length).toBe(KEEP_AWAKE_MODES.length)
    for (const m of KEEP_AWAKE_MODES) {
      const radio = screen.getByTestId(`keep-awake-mode-${m.mode}`) as HTMLInputElement
      expect(group.contains(radio)).toBe(true)
      expect(radio.type).toBe('radio')
      expect(radio.name).toBe('keep-awake-mode')
      expect(radio.classList.contains(SQUARE_RADIO_CLASS)).toBe(true)
      expect(radio.style.borderRadius).toBe('0px')
      expect(screen.getByRole('radio', { name: m.label })).toBe(radio)
    }
    expect(screen.getByRole('radio', { checked: true })).toBe(screen.getByTestId('keep-awake-mode-always'))
    const battery = screen.getByTestId('keep-awake-battery') as HTMLInputElement
    expect(battery.type).toBe('checkbox')
    expect(battery.classList.contains(SQUARE_CHECK_CLASS)).toBe(true)
    expect(battery.style.borderRadius).toBe('0px')
    expect(menu.querySelector('.rounded-full')).toBeNull()
  })

  describe('mug icon', () => {
    const FILLS: { mode: KeepAwakeStatus['mode']; s: KeepAwakeStatus; fill: string }[] = [
      { mode: 'off', s: STATES[0].s, fill: 'none' },
      { mode: 'working', s: STATES[1].s, fill: 'half' },
      { mode: 'always', s: STATES[2].s, fill: 'full' },
    ]

    for (const { mode, s, fill } of FILLS) {
      it(`${mode} draws the mug ${fill === 'none' ? 'empty' : fill}`, async () => {
        expect(s.mode).toBe(mode)
        await renderWith(s)
        const icon = screen.getByTestId('keep-awake-icon')
        expect(icon.getAttribute('data-fill')).toBe(fill)
        const rim = icon.querySelector('[data-testid="keep-awake-rim"]') as Element
        expect(rim).not.toBeNull()
        const liquid = icon.querySelector('[data-testid="keep-awake-liquid"]')
        if (fill === 'none') {
          expect(liquid).toBeNull()
          return
        }
        expect(liquid).not.toBeNull()
        const surface = liquid as Element
        // Primary theme token, never a hex.
        expect(surface.getAttribute('fill')).toBe('var(--color-accent)')
        // Shown only through the rim.
        const clipRef = surface.getAttribute('clip-path')
        expect(clipRef).toMatch(/^url\(#.+\)$/)
        const clipId = (clipRef as string).slice(5, -1)
        const clip = icon.querySelector('[data-testid="keep-awake-rim-clip"]')
        expect(clip).not.toBeNull()
        expect((clip as Element).getAttribute('id')).toBe(clipId)
        const rimCy = Number(rim.getAttribute('cy'))
        const ry = Number(rim.getAttribute('ry'))
        const drop = Number(surface.getAttribute('cy')) - rimCy
        // Full: surface at the rim. Half: dropped one rim radius, so half shows.
        expect(drop).toBe(fill === 'full' ? 0 : ry)
        expect(surface.getAttribute('rx')).toBe(rim.getAttribute('rx'))
        expect(surface.getAttribute('ry')).toBe(rim.getAttribute('ry'))
      })
    }

    it('uses the timer icon stroke, caps, joins and box', async () => {
      render(<TimerButton />)
      const timer = screen.getByTestId('timer-icon')
      await renderWith(STATES[2].s)
      const mug = screen.getByTestId('keep-awake-icon')
      expect(timer.getAttribute('stroke-width')).toBe(String(TOP_BAR_ICON_STROKE_WIDTH))
      for (const attr of ['stroke-width', 'stroke-linecap', 'stroke-linejoin', 'viewBox', 'class']) {
        expect(mug.getAttribute(attr)).toBe(timer.getAttribute(attr))
      }
    })

    it('two mugs on screen get their own rim clip', async () => {
      h.daemonCliGet.mockResolvedValue({ keepAwake: STATES[2].s })
      render(
        <>
          <KeepAwakeButton />
          <KeepAwakeButton />
        </>,
      )
      const icons = await screen.findAllByTestId('keep-awake-icon')
      expect(icons.length).toBe(2)
      const ids = icons.map((i) => (i.querySelector('[data-testid="keep-awake-rim-clip"]') as Element).getAttribute('id'))
      expect(new Set(ids).size).toBe(2)
    })
  })

  it('renders nothing until the server answers (an older server)', async () => {
    h.daemonCliGet.mockRejectedValue(new Error('404 Not Found'))
    const { container } = render(<KeepAwakeButton />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledTimes(1))
    expect(container.innerHTML).toBe('')
  })

  it('sends the chosen mode and shows the daemon answer, never Set up', async () => {
    const button = await renderWith(STATES[0].s)
    const after = status({
      state: 'lid_open_only',
      label: 'Awake (lid open only)',
      detail: NOT_SET_UP,
      lidHeld: false,
      lidSetup: 'needs_setup',
      lidSetupDetail: SET_UP_HINT,
      canSetUp: true,
      canApproveLid: true,
      message: NOT_SET_UP,
    })
    h.daemonCliPost.mockResolvedValue({ success: true, keepAwake: after })
    fireEvent.click(button)
    await act(async () => {
      fireEvent.click(await screen.findByTestId('keep-awake-mode-always'))
    })
    expect(h.daemonCliPost).toHaveBeenCalledTimes(1)
    expect(h.daemonCliPost).toHaveBeenCalledWith('power/keep-awake', { mode: 'always' })
    await waitFor(() => expect(screen.getByTestId('keep-awake-label').textContent).toBe('Awake (lid open only)'))
    expect(screen.getByTestId('keep-awake').getAttribute('aria-label')).toBe('Keep awake: Awake (lid open only)')
    // Not set up is offered, not run.
    expect(screen.getByTestId('keep-awake-setup')).not.toBeNull()
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('power/helper', expect.anything())
  })

  it('a mode change in flight never says an admin dialog is open', async () => {
    const button = await renderWith(STATES[0].s)
    let resolve: (v: unknown) => void = () => {}
    h.daemonCliPost.mockReturnValue(new Promise((r) => (resolve = r)))
    fireEvent.click(button)
    await act(async () => {
      fireEvent.click(await screen.findByTestId('keep-awake-mode-working'))
    })
    expect(keepAwakeEntryFor(primaryScope().id).busy).toBe(true)
    const menu = screen.getByTestId('keep-awake-menu')
    expect(menu.textContent).not.toMatch(/admin|password/i)
    expect(screen.queryByTestId('keep-awake-setting-up')).toBeNull()
    await act(async () => resolve({ success: true, keepAwake: STATES[1].s }))
  })

  it('Also with the lid closed goes to the daemon as a setting, never Set up', async () => {
    const off = STATES.find((x) => x.name === 'lid open only (switch off)')
    if (!off) throw new Error('fixture missing')
    const button = await renderWith(off.s)
    h.daemonCliPost.mockResolvedValue({ success: true, keepAwake: STATES[2].s })
    fireEvent.click(button)
    const lid = (await screen.findByTestId('keep-awake-lid')) as HTMLInputElement
    expect(lid.type).toBe('checkbox')
    expect(lid.classList.contains(SQUARE_CHECK_CLASS)).toBe(true)
    expect(lid.style.borderRadius).toBe('0px')
    expect(lid.checked).toBe(false)
    expect(screen.getByTestId('keep-awake-lid-note').textContent).toBe(
      'Off: Keep awake holds with the lid open only. Lid closed will still sleep.',
    )
    expect(screen.queryByTestId('keep-awake-setup')).toBeNull()
    await act(async () => {
      fireEvent.click(lid)
    })
    expect(h.daemonCliPost).toHaveBeenCalledTimes(1)
    expect(h.daemonCliPost).toHaveBeenCalledWith('power/keep-awake', { lidClosed: true })
    await waitFor(() => expect((screen.getByTestId('keep-awake-lid') as HTMLInputElement).checked).toBe(true))
  })

  it('Set up posts power/helper and shows the answer', async () => {
    const button = await renderWith(STATES[4].s)
    h.daemonCliPost.mockResolvedValue({ success: true, message: 'Power helper set up.', keepAwake: STATES[2].s })
    fireEvent.click(button)
    expect(screen.getByTestId('keep-awake-lid-note').textContent).toBe(
      `Not set up: lid closed will still sleep. ${SET_UP_HINT}`,
    )
    await act(async () => {
      fireEvent.click(await screen.findByTestId('keep-awake-setup'))
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('power/helper', { action: 'setup' })
    await waitFor(() =>
      expect(screen.getByTestId('keep-awake-label').textContent).toBe('Awake, lid closed OK (on power)'),
    )
    expect(keepAwakeEntryFor(primaryScope().id).settingUp).toBe(false)
  })

  it('a Member or a remote client sees who can set it up, and no Set up button', async () => {
    h.remote = true
    const button = await renderWith(STATES[5].s)
    fireEvent.click(button)
    expect(screen.queryByTestId('keep-awake-setup')).toBeNull()
    expect(screen.getByTestId('keep-awake-lid-note').textContent).toBe(
      `Not set up: lid closed will still sleep. ${ASK_AN_ADMIN}`,
    )
  })

  it('Also on battery goes to the daemon (shared with wake)', async () => {
    const button = await renderWith(STATES[2].s)
    h.daemonCliPost.mockResolvedValue({ success: true, keepAwake: { ...STATES[2].s, alsoOnBattery: true } })
    fireEvent.click(button)
    await act(async () => {
      fireEvent.click(await screen.findByTestId('keep-awake-battery'))
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('power/keep-awake', { onBattery: true })
  })

  it('says a remote host is the one kept awake', async () => {
    h.remote = true
    const button = await renderWith(STATES[2].s)
    fireEvent.click(button)
    expect((await screen.findByTestId('keep-awake-menu')).textContent).toContain(
      'This keeps the host awake, not this laptop.',
    )
  })

  // A body that is not `{ keepAwake }` (another route's body on a desynced
  // socket, an error envelope) must leave the button hidden, not crash on
  // `status.mode`.
  for (const [name, body] of [
    ['{}', {}],
    ['null', null],
    ['a usage body', { harnesses: [] }],
    ['a list body', []],
    ['keepAwake without a mode', { keepAwake: { label: 'x', state: 'off' } }],
  ] as Array<[string, unknown]>) {
    it(`stays hidden for ${name}`, async () => {
      h.daemonCliGet.mockResolvedValue(body)
      render(<KeepAwakeButton />)
      await waitFor(() => {
        expect(h.daemonCliGet).toHaveBeenCalledWith('power/status')
        expect(keepAwakeEntryFor(primaryScope().id).error).toBe('Error: power/status: response has no keepAwake')
      })
      expect(keepAwakeEntryFor(primaryScope().id).status).toBeNull()
      expect(screen.queryByTestId('keep-awake')).toBeNull()
    })
  }
})

describe('a Member on the window’s server (0.43.2 Q3: route floor Admin)', () => {
  it('sees Keep awake read-only and sends nothing', async () => {
    const button = await renderWith({ ...STATES[2].s, canChange: false })
    fireEvent.click(button)
    expect((await screen.findByTestId('keep-awake-read-only')).textContent).toBe(
      'Only an Admin or Owner of this server can change Keep awake.',
    )
    for (const id of ['keep-awake-mode-off', 'keep-awake-mode-working', 'keep-awake-mode-always', 'keep-awake-lid', 'keep-awake-battery']) {
      expect([id, (screen.getByTestId(id) as HTMLInputElement).disabled]).toEqual([id, true])
    }
    await act(async () => {
      fireEvent.click(screen.getByTestId('keep-awake-mode-off'))
    })
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('an Admin (canChange) gets live controls and no read-only line', async () => {
    const button = await renderWith({ ...STATES[2].s, canChange: true })
    fireEvent.click(button)
    expect((await screen.findByTestId('keep-awake-mode-off') as HTMLInputElement).disabled).toBe(false)
    expect(screen.queryByTestId('keep-awake-read-only')).toBeNull()
  })
})

describe('parseKeepAwakeBody', () => {
  it('keeps a full status as it is', () => {
    for (const { s } of STATES) {
      expect(parseKeepAwakeBody(JSON.parse(JSON.stringify({ success: true, keepAwake: s })))).toEqual(s)
    }
  })

  it('reads canChange; a daemon before 0.43.2 (no field, Member floor) reads true', () => {
    const { canChange: _drop, ...old } = STATES[0].s
    const parsedOld = parseKeepAwakeBody({ keepAwake: old })
    if (!parsedOld) throw new Error('old body did not parse')
    expect(parsedOld.canChange).toBe(true)
    const parsedMember = parseKeepAwakeBody({ keepAwake: { ...STATES[0].s, canChange: false } })
    if (!parsedMember) throw new Error('member body did not parse')
    expect(parsedMember.canChange).toBe(false)
  })
})
