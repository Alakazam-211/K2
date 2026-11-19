// A menu's lit row stays inside the menu's shape (Rosson 2026-11-14): the
// rows sit --inset-menu in from the edge, and a row's corner runs parallel
// to the menu's: in Glass its radius is the menu's less the inset (the
// phone's hold menu: a 16px box, rows 6px in, 10px corners); in Terminal it
// is cut, never rounded, by the menu's cut less the inset's share of the
// diagonal. These pin the tokens, the rule, the geometry and every menu that
// opts in.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { floatMaterial, MENU_LIST, MENU_ROW } from './float-material'

const repo = join(__dirname, '..', '..', '..', '..')
const strip = (css: string): string => css.replace(/\/\*[\s\S]*?\*\//g, ' ')
const globals = strip(readFileSync(join(repo, 'src/renderer/globals.css'), 'utf8'))
const generated = readFileSync(join(repo, 'src/renderer/styles.generated.css'), 'utf8')
const src = (p: string): string => readFileSync(join(repo, 'src/renderer', p), 'utf8')

function rule(css: string, sel: string): string {
  const at = css.indexOf(`${sel} {`)
  expect(at, `no rule for ${sel}`).toBeGreaterThanOrEqual(0)
  return css.slice(at, css.indexOf('}', at))
}

function tokens(style: string): { radius: Record<string, { $value: string }>; chamfer: Record<string, { $value: string }>; inset: Record<string, { $value: string }> } {
  return JSON.parse(readFileSync(join(repo, 'styles', style, 'tokens.json'), 'utf8'))
}
const px = (v: string): number => {
  const m = /^(-?[\d.]+)px$/.exec(v)
  if (!m) throw new Error(`not a px length: ${v}`)
  return Number(m[1])
}

/** What the rule computes, in px, for one style's tokens. */
function rowShape(style: string, chamferedMenu: boolean): { inset: number; radius: number; menuRadius: number; menuCut: number; cut: number } {
  const t = tokens(style)
  const inset = px(t.inset.menu.$value)
  const menuRadius = px(t.radius.box.$value)
  const menuCut = chamferedMenu && style === 'terminal' ? px(t.chamfer.surface.$value) : 0
  return { inset, menuRadius, menuCut, radius: Math.max(0, menuRadius - inset), cut: Math.max(0, menuCut - 0.5858 * inset) }
}

const SEL = ':not([data-zen-root] *, [data-k2-thread-zen] *)'

describe('menu inset token', () => {
  it('every style declares inset.menu, and the build emits --inset-menu for each', () => {
    for (const style of ['terminal', 'glass', 'glass-classic', 'bezel']) {
      expect([style, px(tokens(style).inset.menu.$value) > 0]).toEqual([style, true])
    }
    expect(readFileSync(join(repo, 'scripts/build-styles.mjs'), 'utf8')).toContain("inset: ['window', 'menu'],")
    expect(generated.match(/--inset-menu: \d+px;/g)?.length).toBeGreaterThanOrEqual(4)
  })

  it('Glass is the phone hold menu: a 16px box, rows 6px in, 10px corners', () => {
    expect(rowShape('glass', false)).toMatchObject({ menuRadius: 16, inset: 6, radius: 10 })
  })

  it('Terminal sits rows 4px in, square', () => {
    expect(rowShape('terminal', true)).toMatchObject({ inset: 4, radius: 0, menuRadius: 0 })
  })
})

describe('the rule', () => {
  it('a row is the menu radius less the inset, and the list pads by the inset', () => {
    const vars = rule(globals, `:where([data-k2-menu]${SEL})`)
    expect(vars).toContain('--k2-menu-row-radius: max(0px, calc(var(--radius-box) - var(--inset-menu)));')
    expect(vars).toContain('--k2-menu-row-cut: max(0px, calc(var(--k2-menu-cut) - 0.5858 * var(--inset-menu)));')
    expect(vars).toContain('--k2-menu-cut: 0px;')
    expect(rule(globals, `:is([data-k2-menu][data-k2-menu-list], [data-k2-menu] [data-k2-menu-list])${SEL}`)).toContain('padding: var(--inset-menu);')
    expect(rule(globals, `[data-k2-menu] [data-k2-menu-row]${SEL}`)).toContain('border-radius: var(--k2-menu-row-radius);')
  })

  it('in Terminal a row is cut parallel to a cut menu and never rounded', () => {
    expect(rule(globals, `html[data-style="terminal"] :where([data-k2-menu][data-k2-chamfer="surface"]${SEL})`)).toContain(
      '--k2-menu-cut: var(--chamfer-surface);',
    )
    const row = rule(globals, `html[data-style="terminal"] [data-k2-menu] [data-k2-menu-row]${SEL}`)
    expect(row).toContain('border-radius: 0;')
    expect(row).toContain('clip-path: polygon(var(--k2-menu-row-cut) 0, calc(100% - var(--k2-menu-row-cut)) 0, 100% var(--k2-menu-row-cut),')
  })

  it('a row\'s keyboard ring (data-k2-focus) takes the row\'s shape', () => {
    expect(rule(globals, `[data-k2-menu] [data-k2-menu-row][data-k2-focus]:focus-visible${SEL}::after`)).toContain(
      'border-radius: var(--k2-menu-row-radius);',
    )
    expect(rule(globals, `html[data-style="terminal"] [data-k2-menu] [data-k2-menu-row][data-k2-focus]:focus-visible${SEL}::after`)).toContain(
      '--k2-ch-cut: var(--k2-menu-row-cut);',
    )
  })

  it('the rule outranks a menu row\'s own Tailwind padding and corners (it is top level, not in a layer)', () => {
    const at = globals.indexOf(`[data-k2-menu] [data-k2-menu-row]${SEL} {`)
    let depth = 0
    for (const ch of globals.slice(0, at)) {
      if (ch === '{') depth++
      else if (ch === '}') depth--
    }
    expect(depth).toBe(0)
  })
})

describe('geometry', () => {
  it('Glass: the row\'s corner is concentric with the menu\'s (radius + inset = menu radius)', () => {
    const g = rowShape('glass', false)
    // One centre for both curves, so the gap between them is the inset all
    // the way round the corner, as it is along the sides.
    expect(g.radius + g.inset).toBe(g.menuRadius)
  })

  it('Terminal: the row\'s cut is parallel to the menu\'s, as far in as the sides are', () => {
    const t = rowShape('terminal', true)
    expect(t.menuCut).toBe(7)
    expect(t.cut).toBeCloseTo(4.66, 2)
    // The menu's diagonal is x + y = cut; the row's, from its corner at
    // (inset, inset), is x + y = 2 * inset + row cut. Their distance apart:
    const apart = (2 * t.inset + t.cut - t.menuCut) / Math.SQRT2
    expect(apart).toBeCloseTo(t.inset, 1)
    // A square row 4px in would reach the menu's diagonal band (x + y = 8 < 7 + 1.41).
    expect(2 * t.inset).toBeLessThan(t.menuCut + Math.SQRT2)
  })

  it('a square Terminal menu keeps square rows', () => {
    expect(rowShape('terminal', false)).toMatchObject({ radius: 0, cut: 0 })
  })
})

describe('menus that opt in', () => {
  it('floatMaterial marks every menu, and only menus', () => {
    expect(floatMaterial('menu')['data-k2-menu']).toBe('')
    expect(floatMaterial('menu', 'frost')['data-k2-menu']).toBe('')
    for (const role of ['popover', 'palette', 'toast', 'dialog'] as const) expect(floatMaterial(role)['data-k2-menu']).toBeUndefined()
    expect(MENU_LIST).toEqual({ 'data-k2-menu-list': '' })
    expect(MENU_ROW).toEqual({ 'data-k2-menu-row': '' })
  })

  it.each([
    ['ContextMenu/ContextMenu.tsx', 2, 1],
    ['ui/OverflowMenu.tsx', 1, 1],
    ['Projects/bar-menu.tsx', 1, 1],
    ['Settings/controls/SettingControls.tsx', 1, 1],
    ['Sidebar/FocusGroupDropdown.tsx', 1, 1],
    ['ui/SearchableAgentList.tsx', 1, 1],
    ['TopBar/ServerSwitcher.tsx', 1, 2],
    ['TopBar/AppMenuPanel.tsx', 1, 1],
    ['Feedback/TicketStatusMenu.tsx', 1, 1],
    ['Feedback/FeedbackItemView.tsx', 1, 1],
    ['common/HeartbeatSessionPicker.tsx', 1, 3],
    ['Settings/sections/ProjectsSection.tsx', 1, 2],
    ['AgentPane/AgentChatPane.tsx', 1, 3],
    ['FileViewerPane/BoardFileName.tsx', 1, 1],
  ])('%s pads its rows in and shapes them', (file, lists, rows) => {
    const s = src(`components/${file}`)
    expect((s.match(/MENU_LIST/g) ?? []).length - 1).toBeGreaterThanOrEqual(lists)
    expect((s.match(/MENU_ROW/g) ?? []).length - 1).toBeGreaterThanOrEqual(rows)
  })

  it('the context menu leaves its padding to the rule and its flyout clears the inset', () => {
    const s = src('components/ContextMenu/ContextMenu.tsx')
    expect(s).not.toMatch(/padding: '4px 0'/)
    expect(s).toContain("const SUB_SIDE = 'calc(100% + var(--inset-menu))'")
    expect(s).toContain("const SUB_EDGE = 'calc(-1 * var(--inset-menu))'")
  })

  it('no menu row draws a square outline that a Terminal cut would clip', () => {
    for (const file of ['Projects/bar-menu.tsx', 'FileViewerPane/BoardFileName.tsx']) {
      expect([file, /-outline-offset-2/.test(src(`components/${file}`))]).toEqual([file, false])
    }
  })
})
