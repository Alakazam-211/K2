// @vitest-environment jsdom
// Who may open a site (PV-12): the chip, Open, the PIN form, "Share with a PIN",
// public's reason, and the no-step swap a K2 tab does (PV-5).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'

const get = vi.fn()
const post = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...a: unknown[]) => get(...a),
  daemonCliPost: (...a: unknown[]) => post(...a),
}))
const openOff = vi.fn()
vi.mock('@/lib/open-off-origin-http', () => ({ openOffOriginHttp: (u: string) => openOff(u) }))
const { features } = vi.hoisted(() => ({ features: { browserPane: true } }))
vi.mock('@/web/features', () => ({ webFeatures: features }))

import { isNestedSiteUrl, openSite, siteModeLabel, siteUrlForTab } from '@/lib/published-sites'
import { SiteChip, SiteVisibilityControls } from './SiteVisibilityControls'
import { DEV_SHARE_WARNING, pinProblem, SitePinForm } from './SitePinForm'

const scope = { id: 'local', hostKey: 'local', isPrimary: true } as unknown as ServerScope

function site(over: Record<string, unknown> = {}): Record<string, unknown> {
  return { label: 'docs', mode: 'private', dev: false, pinSet: false, reason: null, enforced: true, url: 'https://docs.rosson.k2.dev', warnings: [], ...over }
}

beforeEach(() => {
  get.mockReset()
  post.mockReset()
  openOff.mockReset()
  features.browserPane = true
})
afterEach(() => cleanup())

describe('SiteChip', () => {
  it('says the mode, and dev for a dev site', () => {
    const { rerender } = render(<SiteChip mode="private" />)
    expect(screen.getByTestId('site-visibility-chip').textContent).toBe('Private')
    rerender(<SiteChip mode="pin" dev />)
    expect(screen.getByTestId('site-visibility-chip').textContent).toBe('PIN · dev')
    rerender(<SiteChip mode={null} />)
    expect(screen.queryByTestId('site-visibility-chip')).toBeNull()
    expect(siteModeLabel('public')).toBe('Public')
    expect(siteModeLabel('nonsense')).toBeNull()
  })
})

describe('SitePinForm', () => {
  it('checks the PIN before it sends anything', () => {
    expect(pinProblem('12345', '12345')).toContain('at least 6')
    expect(pinProblem('x'.repeat(65), 'x'.repeat(65))).toContain('at most 64')
    expect(pinProblem('123456', '123457')).toContain("don't match")
    expect(pinProblem('123456', '123456')).toBeNull()
    const onDone = vi.fn()
    render(<SitePinForm scope={scope} label="docs" onDone={onDone} onCancel={() => {}} />)
    fireEvent.change(screen.getByTestId('site-pin-input'), { target: { value: '123456' } })
    fireEvent.change(screen.getByTestId('site-pin-again'), { target: { value: 'other1' } })
    expect((screen.getByTestId('site-pin-save') as HTMLButtonElement).disabled).toBe(true)
    expect(post).not.toHaveBeenCalled()
  })

  it('posts the PIN straight to the route, and warns under 8 characters', async () => {
    post.mockResolvedValue(site({ mode: 'pin', pinSet: true }))
    const onDone = vi.fn()
    render(<SitePinForm scope={scope} label="docs" onDone={onDone} onCancel={() => {}} />)
    fireEvent.change(screen.getByTestId('site-pin-input'), { target: { value: '123456' } })
    fireEvent.change(screen.getByTestId('site-pin-again'), { target: { value: '123456' } })
    expect(screen.getByTestId('site-pin-short')).toBeTruthy()
    fireEvent.click(screen.getByTestId('site-pin-save'))
    await waitFor(() => expect(onDone).toHaveBeenCalled())
    expect(post).toHaveBeenCalledWith(scope, 'publish/visibility', { label: 'docs', mode: 'pin', pin: '123456' })
  })

  it('shows a dev server what the person with the PIN can see', () => {
    render(<SitePinForm scope={scope} label="web" dev onDone={() => {}} onCancel={() => {}} />)
    expect(screen.getByTestId('site-pin-dev-warning').textContent).toBe(DEV_SHARE_WARNING)
  })

  it('asks before moving a public site behind the gate, then goes ahead with yes', async () => {
    post.mockRejectedValueOnce(new Error("'docs' is public today. Pass --yes to go ahead."))
    post.mockResolvedValueOnce(site({ mode: 'pin' }))
    const onDone = vi.fn()
    render(<SitePinForm scope={scope} label="docs" onDone={onDone} onCancel={() => {}} />)
    fireEvent.change(screen.getByTestId('site-pin-input'), { target: { value: '12345678' } })
    fireEvent.change(screen.getByTestId('site-pin-again'), { target: { value: '12345678' } })
    fireEvent.click(screen.getByTestId('site-pin-save'))
    await waitFor(() => expect(screen.getByTestId('site-pin-needs-yes').textContent).toBe("'docs' is public today."))
    fireEvent.click(screen.getByTestId('site-pin-save'))
    await waitFor(() => expect(onDone).toHaveBeenCalled())
    expect(post).toHaveBeenLastCalledWith(scope, 'publish/visibility', { label: 'docs', mode: 'pin', pin: '12345678', yes: true })
  })
})

describe('SiteVisibilityControls', () => {
  it('Share with a PIN opens K2 form, and a change reaches the chip', async () => {
    get.mockResolvedValue(site())
    post.mockResolvedValue(site({ mode: 'pin', pinSet: true }))
    const onChanged = vi.fn()
    render(<SiteVisibilityControls scope={scope} label="docs" onChanged={onChanged} />)
    await waitFor(() => expect(screen.getByTestId('site-share-pin')).toBeTruthy())
    expect(screen.queryByTestId('site-pin-form')).toBeNull()
    fireEvent.click(screen.getByTestId('site-share-pin'))
    fireEvent.change(screen.getByTestId('site-pin-input'), { target: { value: '12345678' } })
    fireEvent.change(screen.getByTestId('site-pin-again'), { target: { value: '12345678' } })
    fireEvent.click(screen.getByTestId('site-pin-save'))
    await waitFor(() => expect(onChanged).toHaveBeenCalled())
    expect(screen.getByTestId('site-visibility-chip').getAttribute('data-mode')).toBe('pin')
    expect(screen.queryByTestId('site-pin-form')).toBeNull()
  })

  it('Public asks for a reason; a dev server also warns and says so to the daemon', async () => {
    get.mockResolvedValue(site({ dev: true }))
    post.mockResolvedValue(site({ dev: true, mode: 'public', reason: 'demo' }))
    render(<SiteVisibilityControls scope={scope} label="web" />)
    await waitFor(() => expect(screen.getByTestId('site-make-public')).toBeTruthy())
    fireEvent.click(screen.getByTestId('site-make-public'))
    expect(screen.getByTestId('site-public-dev-warning')).toBeTruthy()
    expect((screen.getByTestId('site-public-save') as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(screen.getByTestId('site-public-reason-input'), { target: { value: 'demo' } })
    fireEvent.click(screen.getByTestId('site-public-save'))
    await waitFor(() => expect(post).toHaveBeenCalled())
    expect(post).toHaveBeenCalledWith(scope, 'publish/visibility', { label: 'web', mode: 'public', reason: 'demo', devIsPublic: true })
    await waitFor(() => expect(screen.getByTestId('site-public-reason').textContent).toContain('demo'))
  })

  it('shows what the daemon warns about, and a refusal as it comes', async () => {
    get.mockResolvedValue(site({ mode: 'pin', warnings: ['docs listens on 0.0.0.0:3000'] }))
    post.mockRejectedValue(new Error('only the owner or an admin can make a site public'))
    render(<SiteVisibilityControls scope={scope} label="docs" />)
    await waitFor(() => expect(screen.getByTestId('site-visibility-warning').textContent).toContain('0.0.0.0'))
    fireEvent.click(screen.getByTestId('site-make-public'))
    fireEvent.change(screen.getByTestId('site-public-reason-input'), { target: { value: 'launch' } })
    fireEvent.click(screen.getByTestId('site-public-save'))
    await waitFor(() => expect(screen.getByTestId('site-visibility-error').textContent).toContain('owner or an admin'))
  })
})

describe('the no-step swap (PV-5)', () => {
  it('asks only about nested K2 sites', async () => {
    expect(isNestedSiteUrl('https://docs.rosson.k2.dev/a')).toBe(true)
    expect(isNestedSiteUrl('https://rosson.k2.dev/')).toBe(false)
    expect(isNestedSiteUrl('https://example.com/')).toBe(false)
    expect(await siteUrlForTab(scope, 'https://example.com/')).toBe('https://example.com/')
    expect(post).not.toHaveBeenCalled()
  })

  it('loads the one-time link the daemon mints', async () => {
    post.mockResolvedValue({ swap: true, url: 'https://docs.rosson.k2.dev/_k2/site/open?code=abc' })
    expect(await siteUrlForTab(scope, 'https://docs.rosson.k2.dev/guide')).toBe('https://docs.rosson.k2.dev/_k2/site/open?code=abc')
    expect(post).toHaveBeenCalledWith(scope, 'publish/open', { url: 'https://docs.rosson.k2.dev/guide' })
  })

  it('keeps the plain address for a public site, or when the daemon refuses', async () => {
    post.mockResolvedValueOnce({ swap: false })
    expect(await siteUrlForTab(scope, 'https://docs.rosson.k2.dev/')).toBe('https://docs.rosson.k2.dev/')
    post.mockRejectedValueOnce(new Error('role_required'))
    expect(await siteUrlForTab(scope, 'https://docs.rosson.k2.dev/')).toBe('https://docs.rosson.k2.dev/')
  })

  it('Open: the desktop hands the plain address to the Browser tab, which swaps it', async () => {
    await openSite(scope, 'https://docs.rosson.k2.dev/')
    expect(openOff).toHaveBeenCalledWith('https://docs.rosson.k2.dev/')
    expect(post).not.toHaveBeenCalled()
  })

  it('Open: the web client opens the tab first, then gives it the link', async () => {
    features.browserPane = false
    const tab = { opener: {}, location: { href: '' } }
    const open = vi.spyOn(window, 'open').mockReturnValue(tab as unknown as Window)
    post.mockResolvedValue({ swap: true, url: 'https://docs.rosson.k2.dev/_k2/site/open?code=abc' })
    await openSite(scope, 'https://docs.rosson.k2.dev/')
    expect(open).toHaveBeenCalledWith('about:blank', '_blank')
    expect(tab.location.href).toBe('https://docs.rosson.k2.dev/_k2/site/open?code=abc')
    expect(tab.opener).toBeNull()
    open.mockRestore()
  })
})
