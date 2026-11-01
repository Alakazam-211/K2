// Ported from Board's own browser checks (a11y), which ran in Chrome only: now in Chromium and WebKit,
// on the readable widget files, against the stand-in `k2` (tests/board/fake-k2.js) with K2's Garden data.
// The keyboard and screen-reader checks: roles, names, focus, menus, Escape, the board tree.
// Each check keeps its own wording; a miss fails the test and lists every check that missed.
/* eslint-disable */
import { expect, test } from '@playwright/test'
import { collector, openBoard, sticky, threeBoards } from './harness'

test('a11y: The keyboard and screen-reader checks: roles, names, focus, menus, Escape, the board tree', async ({ page }) => {
  const p = page
  const { ok, failures, count: checks } = collector()
  await openBoard(p, threeBoards([sticky('a', 400, 100, 'Top right'), sticky('b', 100, 110, 'Top left'), sticky('c', 100, 400, 'Bottom')]), { board: 'b1', objects: 3 })
  const ev = (f, a) => p.evaluate(f, a)
  const active = () => ev(() => { const a = document.activeElement; return { tag: a.tagName, id: a.id, cls: a.className, label: a.getAttribute('aria-label'), role: a.getAttribute('role'), text: a.textContent.slice(0, 40) } })
  const sel = () => ev(() => [...BoardApp.sel])
  const TEXT = '#toolbar .btn[aria-label^="Text"]'

  // Roles and names the screen reader starts from.
  ok('the canvas is named, not read out as every sticky', await ev(() => { const v = document.getElementById('viewport'); return v.getAttribute('role') === 'application' && v.getAttribute('aria-label') === 'Board canvas' }))
  ok('toasts are announced', await ev(() => document.getElementById('toast').getAttribute('role') === 'status'))
  ok('the current tool is pressed', await ev(() => document.querySelector('#toolbar .btn[aria-label^="Select"]').getAttribute('aria-pressed') === 'true'))

  // A button focused from the keyboard: Space and Enter press it (they don't pan, or edit the selection).
  await ev((s) => document.querySelector(s).focus(), TEXT); await p.keyboard.press('Space')
  ok('Space presses a focused tool button', await ev(() => BoardApp.tool) === 'text')
  ok('…and it says so (aria-pressed)', await ev((s) => document.querySelector(s).getAttribute('aria-pressed'), TEXT) === 'true')
  await p.keyboard.press('Escape'); await ev(() => BoardApp.select(['a']))
  await ev(() => document.querySelector('#toolbar .btn[aria-label^="Select"]').focus()); await p.keyboard.press('Enter')
  ok('Enter on a focused button doesn’t open the selected sticky’s editor', !(await ev(() => BoardApp.editing)), await ev(() => BoardApp.editing))

  // After a mouse click the button isn't focus-visible: Space still pans.
  await p.click(TEXT); await ev(() => { BoardApp.setTool('select') })
  const c0 = await ev(() => Object.assign({}, BoardApp.cam))
  await p.keyboard.down(' '); await p.mouse.move(700, 600); await p.mouse.down(); await p.mouse.move(760, 640, { steps: 4 }); await p.mouse.up(); await p.keyboard.up(' ')
  const c1 = await ev(() => Object.assign({}, BoardApp.cam))
  ok('after a mouse click on a button, Space still pans', Math.round(c1.x - c0.x) === 60, [c0, c1])

  // Popovers opened from the keyboard: focus goes in; Escape gives it back to the opener.
  await ev(() => BoardApp.select([])); await ev(() => document.querySelector('#toolbar .btn[aria-label="Templates"]').focus()); await p.keyboard.press('Enter')
  let a = await active()
  ok('a panel opened from the keyboard takes focus', await ev(() => document.getElementById('pop').contains(document.activeElement)), a)
  ok('the panel is a named dialog, and its opener says it’s open', await ev(() => { const pop = document.getElementById('pop'); const o = document.querySelector('#toolbar .btn[aria-label="Templates"]'); return pop.getAttribute('role') === 'dialog' && pop.getAttribute('aria-label') === 'Templates' && o.getAttribute('aria-expanded') === 'true' }))
  await p.keyboard.press('Escape'); a = await active()
  ok('Escape closes it and focus goes back to the opener', a.label === 'Templates' && await ev(() => document.getElementById('pop').hidden), a)
  ok('…and the opener says it’s closed', await ev(() => document.querySelector('#toolbar .btn[aria-label="Templates"]').getAttribute('aria-expanded')) === 'false')

  // The theme picker is a menu: arrows move, Enter picks.
  await ev(() => document.querySelector('#topright .btn[aria-label^="Light or dark"]').focus()); await p.keyboard.press('Enter')
  const m0 = await active()
  ok('the theme picker is a menu and focus starts on the current choice', await ev(() => document.getElementById('pop').getAttribute('role')) === 'menu' && m0.role === 'menuitemradio', m0)
  await p.keyboard.press('ArrowDown'); const m1 = await active(); await p.keyboard.press('ArrowDown'); const m2 = await active()
  ok('ArrowDown moves through the choices', m1.text !== m0.text && m2.text !== m1.text && m2.role === 'menuitemradio', [m0.text, m1.text, m2.text])
  await p.keyboard.press('Escape')

  // Tab on the canvas walks the objects in reading order; past the last, Tab leaves the canvas.
  await ev(() => { BoardApp.select([]); document.getElementById('viewport').focus() })
  await p.keyboard.press('Tab'); const t1 = await sel(); await p.keyboard.press('Tab'); const t2 = await sel(); await p.keyboard.press('Tab'); const t3 = await sel()
  ok('Tab selects objects in reading order (row by row, left to right)', [t1, t2, t3].map((x) => x.join()).join(' ') === 'b a c', [t1, t2, t3])
  await p.keyboard.press('Shift+Tab'); ok('Shift+Tab goes back', (await sel()).join() === 'a')
  await p.waitForTimeout(400)
  ok('the selection is announced', /Sticky note “Top right” selected/.test(await ev(() => document.getElementById('announce').textContent)), await ev(() => document.getElementById('announce').textContent))
  await p.keyboard.press('Tab'); await p.keyboard.press('Tab')
  ok('Tab after the last object leaves the canvas', (await sel()).length === 0 && (await active()).id !== 'viewport', await active())

  // A group is one Tab stop; a mind node reached by Tab doesn't swallow the next Tab (or grow a child).
  await ev(() => BoardApp.model.apply([
    { op: 'add', obj: { id: 'g1', type: 'sticky', x: 100, y: 700, w: 200, h: 200, text: 'G one', color: 'blue', group: 'gg', z: 5 } },
    { op: 'add', obj: { id: 'g2', type: 'sticky', x: 400, y: 700, w: 200, h: 200, text: 'G two', color: 'blue', group: 'gg', z: 6 } },
    { op: 'add', obj: { id: 'mm', type: 'mind', x: 700, y: 760, w: 160, h: 50, text: 'Idea', z: 7 } },
    { op: 'add', obj: { id: 'zz', type: 'sticky', x: 100, y: 1000, w: 200, h: 200, text: 'Last', color: 'green', z: 8 } }]))
  const n0 = await ev(() => BoardApp.model.all().length)
  await ev(() => { BoardApp.select(['c']); document.getElementById('viewport').focus() })
  const walk = []; for (let k = 0; k < 4; k++) { await p.keyboard.press('Tab'); walk.push((await sel()).sort().join('+')) }
  ok('Tab passes a group as one stop, then a mind node, then on', walk.join(' ') === 'g1+g2 mm zz ' || walk.slice(0, 3).join(' ') === 'g1+g2 mm zz', walk)
  ok('…and adds no mind-map child on the way', await ev(() => BoardApp.model.all().length) === n0 && !(await ev(() => BoardApp.editing)))
  await ev(() => BoardApp.model.apply(['g1', 'g2', 'mm', 'zz'].map((id) => ({ op: 'del', id })))); await ev(() => BoardApp.select([]))

  // A panel the mouse opened: Escape hands focus back, and Space still pans, Enter still edits.
  await p.click('#topright .btn[aria-label^="Search"]'); await p.keyboard.type('Top'); await p.waitForTimeout(80)
  const hits = await ev(() => document.querySelectorAll('.found').length)
  await p.keyboard.press('Escape')
  ok('closing the search clears its highlights', hits > 0 && await ev(() => document.querySelectorAll('.found').length) === 0, hits)
  const d0 = await ev(() => Object.assign({}, BoardApp.cam))
  await p.keyboard.down(' '); await p.mouse.move(700, 600); await p.mouse.down(); await p.mouse.move(760, 640, { steps: 4 }); await p.mouse.up(); await p.keyboard.up(' ')
  ok('after Escape from a mouse-opened panel, Space still pans', Math.round((await ev(() => BoardApp.cam.x)) - d0.x) === 60)
  await ev(() => BoardApp.select(['a'])); await p.keyboard.press('Enter')
  ok('…and Enter edits the selected sticky', (await ev(() => BoardApp.editing && BoardApp.editing.id)) === 'a')
  await p.keyboard.press('Escape'); await ev(() => BoardApp.select([]))

  // ⌘F then Escape: focus lands on Search as a keyboard focus, so Enter presses it (not the sticky's editor).
  await ev(() => { BoardApp.select(['a']); document.getElementById('viewport').focus() }); await p.keyboard.press('Meta+f'); await p.waitForTimeout(50); await p.keyboard.press('Escape')
  await p.keyboard.press('Enter'); await p.waitForTimeout(50)
  ok('⌘F, Escape, Enter: presses Search, doesn’t edit the selection', !(await ev(() => BoardApp.editing)) && await ev(() => !document.getElementById('pop').hidden), await active())
  await p.keyboard.press('Escape'); await ev(() => BoardApp.select([]))

  // Tab onto a mind node, then pick it again with the mouse: Tab adds a child again.
  await ev(() => BoardApp.model.apply([{ op: 'add', obj: { id: 'mm', type: 'mind', x: 700, y: 500, w: 160, h: 50, text: 'Idea', z: 9 } }]))
  await ev(() => { BoardApp.select(['b']); document.getElementById('viewport').focus() })
  for (let k = 0; k < 6 && (await sel()).join() !== 'mm'; k++) await p.keyboard.press('Tab')
  const at = async (id) => ev((oid) => { const b = BoardApp.R.box(oid); return BoardApp.toScreen(b.x + b.w / 2, b.y + b.h / 2) }, id)
  let pt = await at('c'); await p.mouse.click(pt[0], pt[1]); pt = await at('mm'); await p.mouse.click(pt[0], pt[1])
  const before = await ev(() => BoardApp.model.all().length); await p.keyboard.press('Tab'); await p.waitForTimeout(50)
  ok('a mind node picked by the mouse after a Tab visit: Tab adds a child', await ev(() => BoardApp.model.all().length) === before + 1, [await sel(), before])
  await p.keyboard.press('Escape'); await ev(() => BoardApp.select([]))
  // Tab onto it, click that same node: Tab adds a child. Shift+Tab never does, nor Tab from a toolbar button.
  await ev(() => { BoardApp.select(['b']); document.getElementById('viewport').focus() })
  for (let k = 0; k < 8 && (await sel()).join() !== 'mm'; k++) await p.keyboard.press('Tab')
  { const n0 = await ev(() => BoardApp.model.all().length)
    await p.keyboard.down(' '); await p.mouse.move(1200, 820); await p.mouse.down(); await p.mouse.move(1180, 800, { steps: 3 }); await p.mouse.up(); await p.keyboard.up(' ')
    await p.keyboard.press('Tab'); await p.waitForTimeout(50)
    ok('a Space-drag pan mid-walk: Tab still moves on (no child added)', await ev(() => BoardApp.model.all().length) === n0 && (await sel()).join() !== 'mm', await sel())
    await ev(() => { BoardApp.select(['b']); document.getElementById('viewport').focus() })
    for (let k = 0; k < 8 && (await sel()).join() !== 'mm'; k++) await p.keyboard.press('Tab') }
  pt = await at('mm'); await p.mouse.click(pt[0], pt[1])
  let n1 = await ev(() => BoardApp.model.all().length); await p.keyboard.press('Tab'); await p.waitForTimeout(50)
  ok('Tab to a mind node, click it, Tab: adds a child', await ev(() => BoardApp.model.all().length) === n1 + 1)
  await p.keyboard.press('Escape'); pt = await at('mm'); await p.mouse.click(pt[0], pt[1])
  n1 = await ev(() => BoardApp.model.all().length); await p.keyboard.press('Shift+Tab'); await p.waitForTimeout(50)
  ok('Shift+Tab on a mind node adds nothing', await ev(() => BoardApp.model.all().length) === n1)
  pt = await at('mm'); await p.mouse.click(pt[0], pt[1]); await p.click('#toolbar .btn[aria-label^="Select"]'); await p.keyboard.press('Tab'); await p.waitForTimeout(50)
  ok('Tab from a toolbar button adds nothing', await ev(() => BoardApp.model.all().length) === n1)
  await p.keyboard.press('Escape'); await ev(() => BoardApp.model.apply(BoardApp.model.all().filter((o) => o.type === 'mind').map((o) => ({ op: 'del', id: o.id })))); await ev(() => BoardApp.select([]))

  // The board tree: one Tab stop, arrows move between rows.
  await ev(() => document.querySelector('.boardname .name').focus()); await p.keyboard.press('Enter'); await p.waitForTimeout(50)
  const stops = await ev(() => [...document.querySelectorAll('#pop .tree .trow')].filter((r) => r.tabIndex === 0).length)
  ok('the board tree is one Tab stop', stops === 1, stops)
  await ev(() => document.querySelector('#pop .tree .trow[tabindex="0"]').focus())
  const r0 = await active(); await p.keyboard.press('ArrowDown'); const r1 = await active(); await p.keyboard.press('ArrowUp'); const r2 = await active()
  ok('ArrowDown and ArrowUp move between rows', r0.text !== r1.text && r2.text === r0.text && r1.role === 'treeitem', [r0.text, r1.text, r2.text])
  await p.keyboard.press('End'); const r3 = await active()
  ok('End goes to the last row', /Third|Second|Test/.test(r3.text) && r3.text !== r0.text, r3)
  await p.keyboard.press('Home'); const f0 = await active(); await p.keyboard.press('ArrowLeft'); const f1 = await active()
  ok('ArrowLeft closes a folder and keeps focus on it', f0.cls.includes('tfold') && f1.cls.includes('tfold') && await ev(() => document.activeElement.getAttribute('aria-expanded')) === 'false', [f0, f1])
  await p.keyboard.press('ArrowDown'); ok('a closed folder’s boards are skipped', (await active()).cls.includes('tfold') || (await ev(() => document.querySelectorAll('#pop .tree .brow').length)) === 0)
  await p.keyboard.press('Home'); await p.keyboard.press('ArrowRight')
  ok('ArrowRight opens it again', await ev(() => document.activeElement.getAttribute('aria-expanded')) === 'true' && await ev(() => document.querySelectorAll('#pop .tree .brow').length) === 3)
  ok('rows carry their level', await ev(() => [...document.querySelectorAll('#pop .tree .trow')].every((r) => Number(r.getAttribute('aria-level')) >= 1)))
  await p.keyboard.press('Escape')

  // Right-click menu: on an object it acts on it; on empty canvas it adds where you clicked; Shift+F10 opens it too.
  const centre = async (id) => ev((oid) => { const b = BoardApp.R.box(oid); return BoardApp.toScreen(b.x + b.w / 2, b.y + b.h / 2) }, id)
  let q = await centre('c'); await p.mouse.click(q[0], q[1], { button: 'right' }); await p.waitForTimeout(50)
  const items = await ev(() => [...document.querySelectorAll('#pop [role=menuitem]')].map((b) => b.querySelector('span').textContent))
  ok('right-click on a sticky selects it and opens its menu', (await sel()).join() === 'c' && await ev(() => document.getElementById('pop').getAttribute('role')) === 'menu' && items.includes('Delete') && items.includes('Bring to front'), items)
  await ev(() => [...document.querySelectorAll('#pop [role=menuitem]')].find((b) => /Copy/.test(b.textContent)).click())
  await p.mouse.click(1100, 760, { button: 'right' }); await p.waitForTimeout(50)
  const empty = await ev(() => [...document.querySelectorAll('#pop [role=menuitem]')].map((b) => b.querySelector('span').textContent))
  ok('right-click on empty canvas: paste, add, select all, fit', (await sel()).length === 0 && ['Paste here', 'Add a sticky note here', 'Select all', 'Fit everything'].every((x) => empty.includes(x)), empty)
  const n3 = await ev(() => BoardApp.model.all().length)
  await ev(() => [...document.querySelectorAll('#pop [role=menuitem]')].find((b) => /Paste here/.test(b.textContent)).click())
  const pasted = await ev(() => { const o = BoardApp.selected()[0]; const [x, y] = BoardApp.toScreen(o.x + o.w / 2, o.y + o.h / 2); return { n: BoardApp.model.all().length, x: Math.round(x), y: Math.round(y), text: o.text } })
  ok('Paste here puts the copy where you right-clicked', pasted.n === n3 + 1 && Math.abs(pasted.x - 1100) < 3 && Math.abs(pasted.y - 760) < 3 && pasted.text === 'Bottom', pasted)
  await p.mouse.click(1100, 300, { button: 'right' }); await p.waitForTimeout(50)
  await ev(() => [...document.querySelectorAll('#pop [role=menuitem]')].find((b) => /Add a sticky/.test(b.textContent)).click()); await p.waitForTimeout(50)
  const added = await ev(() => { const o = BoardApp.selected()[0]; const [x, y] = BoardApp.toScreen(o.x + o.w / 2, o.y + o.h / 2); return { type: o.type, x: Math.round(x), y: Math.round(y), editing: !!BoardApp.editing } })
  ok('Add a sticky note here: at the point, typing', added.type === 'sticky' && Math.abs(added.x - 1100) < 3 && Math.abs(added.y - 300) < 3 && added.editing, added)
  await p.keyboard.type('From the menu'); await p.keyboard.press('Escape'); await p.waitForTimeout(50)
  await ev(() => { BoardApp.select(['c']); document.getElementById('viewport').focus() }); await p.keyboard.press('Shift+F10'); await p.waitForTimeout(50)
  const kf = await active()
  ok('Shift+F10 opens the menu with focus on its first item', kf.role === 'menuitem' && /Copy/.test(kf.text), kf)
  await p.keyboard.press('ArrowUp'); const last = await active()
  ok('…arrow keys wrap to Delete', /Delete/.test(last.text), last)
  await p.keyboard.press('Enter'); await p.waitForTimeout(50)
  ok('…and Enter on Delete deletes it', !(await ev(() => BoardApp.model.get('c'))))
  await ev(() => BoardApp.model.undo()); ok('undo brings it back', !!(await ev(() => BoardApp.model.get('c'))))

  // Right-click menu: it takes focus (canvas keys don't act under it); handles and frame bodies; snap; only from the canvas.
  await ev(() => BoardApp.select([])); q = await centre('a'); await p.mouse.click(q[0], q[1], { button: 'right' }); await p.waitForTimeout(50)
  const y0 = await ev(() => BoardApp.model.get('a').y); await p.keyboard.press('ArrowDown'); await p.keyboard.press('ArrowDown')
  ok('a right-clicked menu has focus: arrows move in it, not the sticky', (await ev(() => BoardApp.model.get('a').y)) === y0 && (await active()).role === 'menuitem' && await ev(() => document.getElementById('pop').getAttribute('aria-label')) === 'Selection menu')
  ok('…the first arrow goes to the first item', /Duplicate/.test((await active()).text), await active())
  await p.keyboard.press('Escape'); q = await centre('a'); await p.mouse.click(q[0], q[1], { button: 'right' }); await p.waitForTimeout(50)
  ok('a right-click shows no focus ring until a key is pressed', await ev(() => { const a = document.activeElement; return a.id === 'pop' && getComputedStyle(a).outlineStyle === 'none' && !document.querySelector('#pop [role=menuitem]:focus') }), await active())
  await p.keyboard.press('ArrowUp'); ok('…and ArrowUp from there goes to the last item (Delete)', /Delete/.test((await active()).text), await active())
  await p.keyboard.press('Escape'); ok('…Escape closes it and keeps the selection', (await sel()).join() === 'a' && await ev(() => document.getElementById('pop').hidden))
  const hd = await ev(() => { const h = [...document.querySelectorAll('#overlay *')].find((x) => x.getBoundingClientRect().width > 0 && x.getBoundingClientRect().width < 20); const r = h.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] })
  const prevented = await ev(([x, y]) => { const t = document.elementFromPoint(x, y); const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: x, clientY: y }); t.dispatchEvent(e); return e.defaultPrevented }, hd)
  ok('right-click on a selection handle opens the Board menu', prevented && await ev(() => !document.getElementById('pop').hidden && document.getElementById('pop').getAttribute('aria-label') === 'Selection menu'))
  await p.keyboard.press('Escape')
  // A connection dot on another object (shown on hover): the menu is for that object, not the selection.
  q = await centre('b'); await p.mouse.move(q[0], q[1]); await p.waitForTimeout(80)
  const dotAt = await ev(() => { const d = [...document.querySelectorAll('#overlay .cdot')].find((x) => x.dataset.id === 'b'); if (!d) return null; const r = d.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] })
  if (dotAt) { await p.mouse.click(dotAt[0], dotAt[1], { button: 'right' }); await p.waitForTimeout(50) }
  ok('right-click on another object’s connection dot: its menu, not the selection’s', dotAt && (await sel()).join() === 'b', [dotAt, await sel()])
  await p.keyboard.press('Escape'); await ev(() => BoardApp.select(['a']))
  await ev(() => { BoardApp.model.apply([{ op: 'add', obj: { id: 'fr', type: 'frame', x: 3000, y: 3000, w: 500, h: 400, title: 'F', z: 0 } }]); BoardApp.animateTo({ s: 1, x: 700 - 3250, y: 450 - 3200 }, 0) })   // an empty part of the board
  q = await ev(() => BoardApp.toScreen(3250, 3250)); await p.mouse.click(q[0], q[1], { button: 'right' }); await p.waitForTimeout(50)
  ok('right-click inside a frame’s empty body is the canvas menu', (await sel()).length === 0 && await ev(() => /Add a sticky/.test(document.getElementById('pop').textContent)))
  await ev(() => { BoardApp.snap = true }); await ev(() => [...document.querySelectorAll('#pop [role=menuitem]')].find((b) => /Add a sticky/.test(b.textContent)).click()); await p.keyboard.press('Escape')
  const sn = await ev(() => { const o = BoardApp.model.all().filter((x) => x.type === 'sticky').pop(); return [o.x % 24, o.y % 24] })
  ok('Add a sticky here lands on the grid when snap is on', sn[0] === 0 && sn[1] === 0, sn)
  await ev(() => { BoardApp.snap = false; BoardApp.model.apply([{ op: 'del', id: 'fr' }]); BoardApp.select(['a']) })
  await ev(() => document.querySelector('#toolbar .btn[aria-label^="Select"]').focus()); await p.keyboard.press('Shift+F10')
  ok('Shift+F10 on a toolbar button doesn’t open the canvas menu', await ev(() => document.getElementById('pop').hidden))

  // Shortcut list names the keyboard keys.
  ok('the shortcut list has Tab, Enter and Esc', await ev(() => { document.querySelector('#zoombar .btn[aria-label^="Keyboard shortcuts"]').click(); const t = document.getElementById('pop').textContent; return /Tab · ⇧Tab/.test(t) && /Enter/.test(t) && /Esc/.test(t) }))

  ok('no page errors', (await p.evaluate(() => (window as any).__errors)).length === 0, await p.evaluate(() => (window as any).__errors))
  expect(failures(), checks() + ' checks').toEqual([])
})
