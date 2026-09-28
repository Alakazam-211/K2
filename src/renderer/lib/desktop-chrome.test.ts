import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { APP_MENU_ACTION_IDS } from './app-menu-actions'
import {
  APP_MENU_BUTTON_MIN_WIDTH_PX,
  desktopChromeFor,
  desktopOsFromNavigator,
  logoOpensAppMenu,
  topBarLeftClusterMinWidth,
  TRAFFIC_LIGHT_SPACER_BASE_PX,
  type DesktopChrome,
} from './desktop-chrome'
import {
  LINUX_STOPLIGHT_ACTIVE,
  LINUX_STOPLIGHT_GAP_PX,
  LINUX_STOPLIGHT_HIT_PX,
  LINUX_STOPLIGHT_INACTIVE_ALPHA,
  LINUX_STOPLIGHT_INACTIVE_RGB,
  LINUX_STOPLIGHT_RADIUS_PX,
  LINUX_STOPLIGHT_ROLES,
  LINUX_STOPLIGHT_SQUARE_PX,
  linuxStoplightFill,
} from './linux-stoplights'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..')

const NONE: DesktopChrome = {
  trafficLightSpacer: false,
  appMenuButton: false,
  windowControls: false,
  linuxStoplights: false,
}

function read(rel: string): string {
  return readFileSync(resolve(root, rel), 'utf8')
}

describe('desktop chrome split', () => {
  it('keeps macOS on the system spacer and off the in-app controls', () => {
    expect(desktopChromeFor(false, 'mac')).toEqual({
      ...NONE,
      trafficLightSpacer: true,
    })
  })

  it('gives Windows window controls and no menu button or stoplights', () => {
    expect(desktopChromeFor(false, 'windows')).toEqual({
      ...NONE,
      windowControls: true,
    })
  })

  it('gives Linux in-flow squares and not a spacer or window controls', () => {
    expect(desktopChromeFor(false, 'linux')).toEqual({
      ...NONE,
      linuxStoplights: true,
    })
    expect(desktopChromeFor(false, 'linux').trafficLightSpacer).toBe(false)
    expect(desktopChromeFor(false, 'linux').appMenuButton).toBe(false)
    expect(desktopChromeFor(false, 'linux').windowControls).toBe(false)
  })

  it('leaves hosted web with every flag false, on every OS', () => {
    for (const os of ['mac', 'windows', 'linux', 'other'] as const) {
      expect(desktopChromeFor(true, os)).toEqual(NONE)
    }
  })

  it('does not share one Win/Linux return anymore', () => {
    const win = desktopChromeFor(false, 'windows')
    const linux = desktopChromeFor(false, 'linux')
    expect(win).not.toEqual(linux)
    expect(win.appMenuButton).toBe(false)
    expect(linux.appMenuButton).toBe(false)
  })

  it('detects mac, windows, and linux from platform or user agent', () => {
    expect(desktopOsFromNavigator({ platform: 'MacIntel', userAgent: '' })).toBe('mac')
    expect(
      desktopOsFromNavigator({
        platform: '',
        userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)',
      }),
    ).toBe('mac')
    expect(desktopOsFromNavigator({ platform: 'Win32', userAgent: 'Windows NT 10.0' })).toBe(
      'windows',
    )
    expect(
      desktopOsFromNavigator({ platform: 'Linux x86_64', userAgent: 'X11; Linux x86_64' }),
    ).toBe('linux')
    expect(desktopOsFromNavigator({ platform: '', userAgent: 'X11; Linux x86_64' })).toBe('linux')
    expect(desktopOsFromNavigator(null)).toBe('other')
  })
})

describe('logo menu vs dashboard', () => {
  it('opens the menu from the icon on Windows and Linux only', () => {
    expect(logoOpensAppMenu(false, 'windows')).toBe(true)
    expect(logoOpensAppMenu(false, 'linux')).toBe(true)
    expect(logoOpensAppMenu(false, 'other')).toBe(true)
    expect(logoOpensAppMenu(false, 'mac')).toBe(false)
    expect(logoOpensAppMenu(true, 'windows')).toBe(false)
    expect(logoOpensAppMenu(true, 'linux')).toBe(false)
    expect(logoOpensAppMenu(true, 'mac')).toBe(false)
  })

  it('anchors the panel on the icon, not a left-slot Menu button', () => {
    const mark = read('src/renderer/components/TopBar/K2MarkButton.tsx')
    const panel = read('src/renderer/components/TopBar/AppMenuPanel.tsx')
    const left = read('src/renderer/components/TopBar/DesktopChromeLeft.tsx')
    expect(mark).toContain('className="relative flex-shrink-0 no-drag"')
    expect(mark).toContain('<AppMenuPanel')
    expect(mark).toContain('className="h-4 w-4"')
    expect(mark).toContain('https://k2.dev/dashboard')
    expect(mark).toContain('logoOpensAppMenu')
    expect(panel).toContain('absolute left-0 top-full')
    expect(panel).not.toMatch(/className="[^"]*\brelative\b/)
    expect(left).toContain('chrome.linuxStoplights')
    expect(left).toContain('<LinuxStoplights />')
    expect(left).not.toContain('AppMenuButton')
    expect(left).not.toContain('set_traffic_light_inset')
  })
})

describe('left cluster min width', () => {
  it('matches the Agents bar: spacer, retired Menu button, otherwise none', () => {
    expect(topBarLeftClusterMinWidth(desktopChromeFor(false, 'mac'))).toBe(
      TRAFFIC_LIGHT_SPACER_BASE_PX + 60,
    )
    expect(
      topBarLeftClusterMinWidth({
        ...NONE,
        appMenuButton: true,
      }),
    ).toBe(APP_MENU_BUTTON_MIN_WIDTH_PX + 60)
    expect(topBarLeftClusterMinWidth(desktopChromeFor(false, 'windows'))).toBeUndefined()
    expect(topBarLeftClusterMinWidth(desktopChromeFor(false, 'linux'))).toBeUndefined()
    expect(topBarLeftClusterMinWidth(NONE)).toBeUndefined()
  })
})

describe('linux stoplight paint', () => {
  it('orders close, minimize, maximize as small squares', () => {
    expect([...LINUX_STOPLIGHT_ROLES]).toEqual(['close', 'minimize', 'maximize'])
    expect(LINUX_STOPLIGHT_RADIUS_PX).toBe(0)
    expect(LINUX_STOPLIGHT_SQUARE_PX).toBeLessThan(LINUX_STOPLIGHT_HIT_PX)
    expect(LINUX_STOPLIGHT_HIT_PX).toBe(14)
    expect(LINUX_STOPLIGHT_SQUARE_PX).toBe(12)
    expect(LINUX_STOPLIGHT_GAP_PX).toBe(9)
  })

  it('uses active_rgb when focused and inactive_rgb at alpha 0.2 when not', () => {
    expect(linuxStoplightFill('close', { focused: true, dark: true })).toBe('#FF5F57')
    expect(linuxStoplightFill('minimize', { focused: true, dark: false })).toBe('#FEBC2E')
    expect(linuxStoplightFill('maximize', { focused: true, dark: true })).toBe('#28C840')
    expect(LINUX_STOPLIGHT_ACTIVE).toEqual({
      close: '#FF5F57',
      minimize: '#FEBC2E',
      maximize: '#28C840',
    })

    expect(LINUX_STOPLIGHT_INACTIVE_RGB.dark).toEqual({ r: 0.3, g: 0.3, b: 0.3 })
    expect(LINUX_STOPLIGHT_INACTIVE_RGB.light).toEqual({ r: 0.84, g: 0.84, b: 0.84 })
    expect(LINUX_STOPLIGHT_INACTIVE_ALPHA).toBe(0.2)

    const dark = linuxStoplightFill('close', { focused: false, dark: true })
    const light = linuxStoplightFill('minimize', { focused: false, dark: false })
    expect(dark).toBe('color(srgb 0.3 0.3 0.3 / 0.2)')
    expect(light).toBe('color(srgb 0.84 0.84 0.84 / 0.2)')
    expect(dark).not.toBe(light)
    for (const role of LINUX_STOPLIGHT_ROLES) {
      const fill = linuxStoplightFill(role, { focused: false, dark: true })
      expect(fill).toBe(dark)
      expect(fill).not.toContain('FF5F57')
      expect(fill).not.toContain('FEBC2E')
      expect(fill).not.toContain('28C840')
      expect(fill).not.toContain('255, 95, 87')
    }
  })

  it('does not call the macOS inset command or paint circles', () => {
    const src = read('src/renderer/components/TopBar/LinuxStoplights.tsx')
    expect(src).not.toContain('set_traffic_light_inset')
    expect(src).not.toContain('rounded-full')
    expect(src).not.toContain('border-radius: 50%')
    expect(src).toContain('LINUX_STOPLIGHT_ROLES.map')
    expect(src).toContain('borderRadius: LINUX_STOPLIGHT_RADIUS_PX')
    expect(src).toContain('.minimize()')
    expect(src).toContain('.unmaximize()')
    expect(src).toContain('.maximize()')
    expect(src).toContain('.close()')
  })
})

describe('top bars share the Agents bar, not Focus', () => {
  const pages = [
    'src/renderer/components/Projects/ProjectsPage.tsx',
    'src/renderer/components/Feedback/FeedbackPage.tsx',
    'src/renderer/components/Settings/Settings.tsx',
    'src/renderer/components/Wiki/WikiPage.tsx',
  ]

  it('replaces the K2 wordmark with the logo and the utility cluster', () => {
    for (const rel of pages) {
      const src = read(rel)
      expect(src, rel).toContain('<K2MarkButton />')
      expect(src, rel).toContain('<TopBarUtilities')
      expect(src, rel).toContain('[&>*]:shrink-0')
      expect(src, rel).toContain('topBarLeftClusterMinWidth()')
      expect(src, rel).toContain('<PageTabs />')
      expect(src, rel).not.toContain('tracking-widest text-[var(--color-text-muted)] uppercase')
    }
    const agents = read('src/renderer/components/TopBar/TopBar.tsx')
    expect(agents).toContain('<K2MarkButton />')
    expect(agents).toContain('<TopBarUtilities')
    expect(agents).toContain('title="Run workspace command"')
    expect(read('src/renderer/components/Projects/ProjectsPage.tsx')).not.toContain(
      'Run workspace command',
    )
  })

  it('puts Projects nav and chat, and Tickets and Wiki close, after the utilities start', () => {
    const projects = read('src/renderer/components/Projects/ProjectsPage.tsx')
    const navAt = projects.indexOf('title="Toggle projects nav"')
    const chatAt = projects.indexOf('title="Toggle project chat"')
    const utilsAt = projects.indexOf('<TopBarUtilities>')
    const utilsEnd = projects.indexOf('</TopBarUtilities>')
    expect(utilsAt).toBeGreaterThan(-1)
    expect(navAt).toBeGreaterThan(utilsAt)
    expect(chatAt).toBeGreaterThan(navAt)
    expect(chatAt).toBeLessThan(utilsEnd)

    const tickets = read('src/renderer/components/Feedback/FeedbackPage.tsx')
    const ticketUtils = tickets.indexOf('<TopBarUtilities>')
    expect(tickets.indexOf('title="Close (Esc)"')).toBeGreaterThan(ticketUtils)
    expect(tickets).toContain('h-6 w-6')
    expect(tickets).not.toContain('w-7 h-7')

    const wiki = read('src/renderer/components/Wiki/WikiPage.tsx')
    const wikiUtils = wiki.indexOf('<TopBarUtilities>')
    const wikiLabel = wiki.search(/truncate ml-2">\s*Wiki/)
    expect(wikiLabel).toBeGreaterThan(-1)
    expect(wikiLabel).toBeLessThan(wikiUtils)
    expect(wiki.indexOf('onClick={closeWiki}')).toBeGreaterThan(wikiUtils)
    expect(wiki).not.toContain('w-7 h-7')
  })

  it('paints Settings and Focus with Surface, and keeps the gate to left chrome', () => {
    const settings = read('src/renderer/components/Settings/Settings.tsx')
    const focus = read('src/renderer/components/Layout/FocusLayout.tsx')
    const gate = read('src/renderer/components/TopBar/GateChrome.tsx')
    expect(settings).toContain('<Surface')
    expect(settings).toContain('role2="surface"')
    expect(focus).toContain('<Surface')
    expect(focus).toContain('role2="surface"')
    expect(focus).toContain('<K2MarkButton />')
    expect(focus).toContain('<TopBarUtilities>')
    expect(focus).not.toContain('PageTabs')
    expect(gate).toContain('<K2MarkButton />')
    expect(gate).toContain('<DesktopChromeLeft />')
    expect(gate).not.toContain('TopBarUtilities')
    expect(gate).not.toContain('PresenceRoster')
    expect(gate).not.toContain('UsageButton')
    expect(gate).not.toContain('TimerButton')
    expect(gate).not.toContain('K2NounsCheatSheet')
    expect(gate).not.toContain('ModeToggle')
  })

  it('keeps every menu action, with New Window first and no scroll cap', () => {
    const panel = read('src/renderer/components/TopBar/AppMenuPanel.tsx')
    for (const id of APP_MENU_ACTION_IDS) {
      expect(panel, id).toContain(`id: '${id}'`)
    }
    expect(panel.match(/id: 'new-window'/g)).toEqual(["id: 'new-window'"])
    expect(panel.indexOf("id: 'new-window'")).toBeLessThan(panel.indexOf("id: 'settings'"))
    expect(panel).not.toMatch(/max-h-/)
    expect(panel).not.toContain('70vh')
    expect(panel).not.toContain('overflow-y-auto')
    expect(panel).not.toContain('480')
  })
})
