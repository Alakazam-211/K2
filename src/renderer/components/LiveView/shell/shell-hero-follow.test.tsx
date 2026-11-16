// @vitest-environment jsdom
// What a view's hero gets from the shell for Follow and Say (T5, shared by
// the scene, the timeline and the editors): whom you follow with the
// follow state and its dispatch, so the view can follow a playhead and
// look away on its own keys and scrolling; and a way to say what the say
// row points at (the timeline: the clip under its playhead), which the
// message then carries as its address and time.

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { useEffect } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { SampleShellSource } from '@/lib/live-view/shared/sample-shell'
import { shellSample } from '@/lib/live-view/shared/samples'
import { LiveShellView } from './LiveShellView'
import type { ShellHeroProps } from './shell-config'
import { SCENE_SHELL } from '../scene/scene-shell'

afterEach(cleanup)

describe('the hero’s Follow and Say hooks', () => {
  it('the hero sees whom you follow and can look away; Back follows again', () => {
    const src = new SampleShellSource(shellSample('scene'), { autoplay: false, scenario: 'people' })
    function Hero(p: ShellHeroProps): React.JSX.Element {
      return (
        <div data-testid="lv-hero">
          <span data-testid="following">{p.follow?.id ?? 'nobody'}</span>
          <span data-testid="kind">{p.follow?.state.kind}</span>
          <button type="button" data-testid="look" onClick={() => p.follow?.dispatch({ type: 'look-away' })} />
          {p.back}
        </div>
      )
    }
    render(<LiveShellView source={src} config={{ ...SCENE_SHELL, backInHeader: false, Hero }} width={1440} />)
    expect(screen.getByTestId('following').textContent).toBe('nobody')
    fireEvent.click(screen.getByTestId('lv-follow'))
    const who = screen.getByTestId('following').textContent
    expect(who).not.toBe('nobody')
    fireEvent.click(screen.getByTestId('look'))
    expect(screen.getByTestId('kind').textContent).toBe('away')
    expect(screen.getByTestId('following').textContent).toBe('nobody')
    fireEvent.click(screen.getByTestId('lv-back'))
    expect(screen.getByTestId('following').textContent).toBe(who)
  })

  it('what the hero says the say row points at is the chip, and the message carries it as its address and time', async () => {
    const src = new SampleShellSource(shellSample('scene'), { autoplay: false, scenario: 'people' })
    const say = vi.spyOn(src, 'say')
    function Hero(p: ShellHeroProps): React.JSX.Element {
      const set = p.setSayAt
      useEffect(() => set?.({ label: 'Interview C', title: 'Interview C at 01:00:44:09', address: '@item:5', time: { at: '1234', in: '@seq:S1' } }), [set])
      return <div data-testid="lv-hero">{p.say}</div>
    }
    render(<LiveShellView source={src} config={{ ...SCENE_SHELL, sayAlways: true, placesSayAndPanel: true, Hero }} width={1440} />)
    await act(async () => {})
    expect(screen.getByTestId('lv-say-chip').textContent).toBe('Interview C')
    expect(screen.getByTestId('lv-say-chip').getAttribute('title')).toContain('Interview C at 01:00:44:09')
    fireEvent.change(screen.getByTestId('lv-say-field'), { target: { value: 'Is this the take?' } })
    fireEvent.keyDown(screen.getByTestId('lv-say-field'), { key: 'Enter' })
    expect(say).toHaveBeenCalledWith(expect.objectContaining({ text: 'Is this the take?', address: '@item:5', time: { at: '1234', in: '@seq:S1' } }))
  })

  it('the hero’s own actions (a card’s Open saved versions) reach the view’s notice handler', () => {
    const src = new SampleShellSource(shellSample('scene'), { autoplay: false, scenario: 'people' })
    const seen: string[] = []
    function Hero(p: ShellHeroProps): React.JSX.Element {
      return <button type="button" data-testid="open-saved" onClick={() => p.onAction?.('open-saved')} />
    }
    render(<LiveShellView source={src} config={{ ...SCENE_SHELL, Hero, onNoticeAction: (id, n) => seen.push(`${id}:${n ? n.kind : 'card'}`) }} width={1440} />)
    fireEvent.click(screen.getByTestId('open-saved'))
    expect(seen).toEqual(['open-saved:card'])
  })
})
