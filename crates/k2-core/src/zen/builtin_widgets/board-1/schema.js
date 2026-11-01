// Board schema (data format 1): object types, fields, validation, colours and templates.
// The agent CLI (`k2 board`) follows the same rules, so both sides accept exactly the same data.
// Nothing here is ever cut or skipped silently: a value that doesn't fit is refused with a typed
// error {code, field, max?} (codes: unknown_type, unknown_field, bad_field, too_long, too_many,
// value_too_large). Numbers round to 0.1 (the canonical precision).
(function (g) {
  'use strict'

  const FORMAT = 1
  // Sticky colours: [light, dark]. Muted in dark so they don't glare.
  const STICKY = {
    yellow: ['#fff1a8', '#d9c96a'], orange: ['#ffd8b0', '#d9a671'], pink: ['#ffd0e2', '#d895ad'],
    red: ['#ffb8b3', '#d4847e'], violet: ['#ddd0ff', '#a996d9'], blue: ['#c4e0ff', '#7fa9d6'],
    cyan: ['#bdf0ea', '#78bdb4'], green: ['#cdeec4', '#8fbf84'], lime: ['#e8f6ae', '#b3c46f'],
    gray: ['#e6e4df', '#a9a6a0'], white: ['#ffffff', '#d8d6d2'],
  }
  // Ink colours for shapes, lines, text and pen: [light, dark].
  const INK = {
    ink: ['#1f1d1a', '#ecebe8'], gray: ['#8a857c', '#a8a39a'], red: ['#e5484d', '#ff6b70'],
    orange: ['#f08c2e', '#ffa557'], yellow: ['#f5c400', '#ffd84d'], green: ['#2f9e5b', '#4cc27b'],
    teal: ['#12a594', '#3cc9b8'], blue: ['#2f6fed', '#6e9bff'], violet: ['#7c5ce6', '#a68bff'],
    pink: ['#d6409f', '#f070c0'], white: ['#ffffff', '#ffffff'],
  }
  // Fills for shapes and frames: [light, dark]. 'none' is transparent.
  const FILL = {
    white: ['#ffffff', '#2a2a2c'], gray: ['#efede8', '#38373a'], red: ['#ffe1df', '#4a2b2b'],
    orange: ['#ffe9d3', '#4a3523'], yellow: ['#fff6c7', '#48411f'], green: ['#dff3d8', '#263d2a'],
    teal: ['#d5f3ee', '#1f3d39'], blue: ['#dde9ff', '#24334f'], violet: ['#ebe4ff', '#342b4f'],
    pink: ['#ffe0ef', '#4a2a3c'], ink: ['#1f1d1a', '#ecebe8'],
  }

  const SHAPES = ['rect', 'round', 'ellipse', 'triangle', 'diamond', 'parallelogram', 'trapezoid', 'pentagon',
    'hexagon', 'octagon', 'star', 'cross', 'arrow-right', 'arrow-left', 'chevron', 'cylinder', 'cloud', 'speech',
    'document', 'flow-start', 'flow-process', 'flow-decision', 'flow-data', 'flow-predefined']
  const CAPS = ['none', 'arrow', 'triangle', 'circle', 'diamond', 'bar']
  const ROUTES = ['straight', 'elbow', 'curved']
  const DASH = ['solid', 'dashed', 'dotted']
  const ANCHORS = ['auto', 'top', 'right', 'bottom', 'left']

  // List caps (too_many past these).
  const LIMITS = { tags: 200, points: 5000, rows: 100, cols: 30, checks: 50 }
  // A stored value (one key) is at most this many bytes of JSON.
  const VALUE_MAX = 60 * 1024

  // Field kinds: num, bool, str:N (max length), enum [...], sticky, ink, fill, list, points, rows, ids,
  // end, checks, replies, reactions, votes, crop, link, src, json.
  const COMMON = {
    x: 'num', y: 'num', w: 'num', h: 'num', rot: 'num', z: 'num',
    locked: 'bool', group: 'str:40', tags: 'list', reactions: 'reactions', votes: 'votes',
    link: 'link',   // an http(s) address shown on the object (null or absent: none)
  }
  const TYPES = {
    sticky: { text: 'str:5000', color: 'sticky', fontSize: 'num', align: ['left', 'center', 'right'] },
    text: { text: 'str:20000', color: 'ink', fontSize: 'num', bold: 'bool', italic: 'bool', align: ['left', 'center', 'right'], checks: 'checks' },
    shape: { shape: SHAPES, text: 'str:5000', fill: 'fill', stroke: 'ink', strokeWidth: 'num', dash: DASH, color: 'ink', fontSize: 'num' },
    connector: { from: 'end', to: 'end', route: ROUTES, stroke: 'ink', strokeWidth: 'num', dash: DASH, startCap: CAPS, endCap: CAPS, label: 'str:500' },
    pen: { points: 'points', stroke: 'ink', strokeWidth: 'num', highlighter: 'bool' },
    frame: { title: 'str:200', fill: 'fill', order: 'num' },
    card: { title: 'str:500', text: 'str:5000', color: 'ink', assignee: 'str:80', due: 'str:40', status: 'str:40', checks: 'checks' },
    table: { rows: 'rows', rowIds: 'rowIds', colIds: 'colIds', colW: 'list', fill: 'fill' },
    mind: { text: 'str:1000', parent: 'str:40', color: 'ink' },
    comment: { text: 'str:5000', replies: 'replies', resolved: 'bool', target: 'str:40' },
    image: { src: 'src', alt: 'str:300', crop: 'crop' },
  }
  const DEFAULTS = {
    sticky: { w: 200, h: 200, text: '', color: 'yellow', fontSize: 0, align: 'center' },
    text: { w: 240, h: 40, text: 'Text', color: 'ink', fontSize: 24, align: 'left' },
    shape: { w: 180, h: 120, shape: 'round', text: '', fill: 'white', stroke: 'ink', strokeWidth: 2, dash: 'solid', color: 'ink', fontSize: 18 },
    connector: { w: 0, h: 0, route: 'elbow', stroke: 'ink', strokeWidth: 2, dash: 'solid', startCap: 'none', endCap: 'arrow', label: '' },
    pen: { w: 1, h: 1, points: [], stroke: 'ink', strokeWidth: 3, highlighter: false },
    frame: { w: 960, h: 600, title: 'Frame', fill: 'white', order: 0 },
    card: { w: 280, h: 150, title: 'Card', text: '', color: 'blue', assignee: '', due: '', status: '' },
    table: { w: 480, h: 160, rows: [['', '', ''], ['', '', ''], ['', '', '']], colW: [], fill: 'white' },
    mind: { w: 180, h: 48, text: 'Idea', parent: '', color: 'violet' },
    comment: { w: 32, h: 32, text: '', replies: [], resolved: false, target: '' },
    image: { w: 320, h: 240, src: '', alt: '' },
  }
  const PREFIX = { sticky: 's', text: 't', shape: 'h', connector: 'c', pen: 'p', frame: 'f', card: 'k', table: 'b', mind: 'm', comment: 'q', image: 'i' }
  const META = { voting: 'json', timer: 'json', title: 'str:200', format: 'format' }
  const REACTIONS = ['👍', '❤️', '😂', '🎉', '🤔', '👀', '🔥', '✅', '❌', '❓', '⭐']
  const STAMPS = ['✅', '❌', '❓', '⭐']   // reactions drawn big, as stamps on the object's corner

  // Ids of list items inside one object (checklist items, replies, table rows and columns).
  const ITEM_ID = { checks: /^ck-[0-9a-z]{1,16}$/, replies: /^rp-[0-9a-z]{1,16}$/, rowIds: /^rw-[0-9a-z]{1,16}$/, colIds: /^cl-[0-9a-z]{1,16}$/ }
  // A new random item id: <prefix>-<8 hex>.
  function itemId(prefix) {
    const a = new Uint8Array(4); g.crypto.getRandomValues(a)
    return prefix + '-' + Array.from(a, (b) => b.toString(16).padStart(2, '0')).join('')
  }
  const BLOB_REF = /^k2blob:sha256-[0-9a-f]{64}$/
  const DATA_IMAGE = /^data:image\/(png|jpeg|webp|gif);base64,[A-Za-z0-9+/=]+$/
  const isBlobRef = (v) => typeof v === 'string' && BLOB_REF.test(v)
  const isDataImage = (v) => typeof v === 'string' && DATA_IMAGE.test(v)

  // A typed refusal: {code, field, max?}. The message is for logs; the UI words its own.
  function refusal(code, field, max, why) {
    const e = new Error((field ? field + ': ' : '') + code + (max != null ? ' (at most ' + max + ')' : '') + (why ? ': ' + why : ''))
    e.name = 'BoardRefusal'; e.code = code; e.field = field || null
    if (max != null) e.max = max
    return e
  }
  const isRefusal = (e) => !!e && e.name === 'BoardRefusal' && typeof e.code === 'string'
  // Characters as people (and the Rust CLI) count them: code points, not UTF-16 units.
  const chars = (s) => { let n = 0; for (const _ of s) n++; return n }
  const isHex = (v) => typeof v === 'string' && /^#[0-9a-fA-F]{6}$/.test(v)
  const isObj = (v) => !!v && typeof v === 'object' && !Array.isArray(v)
  const strOf = (v, key, max) => {
    if (v == null) return ''
    if (typeof v === 'number' && Number.isFinite(v)) v = String(v)
    if (typeof v !== 'string') throw refusal('bad_field', key, null, 'text')
    if (chars(v) > max) throw refusal('too_long', key, max)
    return v
  }
  const numOf = (v, key) => {
    const n = typeof v === 'number' ? v : typeof v === 'string' && v.trim() !== '' ? Number(v) : NaN
    if (!Number.isFinite(n)) throw refusal('bad_field', key, null, 'a number')
    return n
  }

  function check(kind, v, key) {
    const bad = (why) => { throw refusal('bad_field', key, null, why) }
    if (Array.isArray(kind)) { if (!kind.includes(v)) bad('one of ' + kind.join(', ')); return v }
    if (kind === 'num') return Math.round(numOf(v, key) * 10) / 10
    if (kind === 'bool') { if (v === 'true') return true; if (v === 'false') return false; if (typeof v !== 'boolean') bad('true or false'); return v }
    if (kind.startsWith('str:')) return strOf(v, key, Number(kind.slice(4)))
    if (kind === 'sticky') { if (!STICKY[v]) bad('a sticky colour: ' + Object.keys(STICKY).join(', ')); return v }
    if (kind === 'ink') { if (!INK[v] && !isHex(v)) bad('a colour: ' + Object.keys(INK).join(', ') + ' or #rrggbb'); return v }
    if (kind === 'fill') { if (v !== 'none' && !FILL[v] && !isHex(v)) bad('a fill: none, ' + Object.keys(FILL).join(', ') + ' or #rrggbb'); return v }
    if (kind === 'format') { if (!Number.isInteger(v) || v < 1) bad('a format number'); return v }
    if (kind === 'link') {
      if (v == null || v === '') return null
      if (typeof v !== 'string') bad('an http:// or https:// address')
      const u = v.trim()
      if (chars(u) > 2000) throw refusal('too_long', key, 2000)
      if (!/^https?:\/\/[^\s]+$/i.test(u)) bad('an http:// or https:// address')
      return u
    }
    if (kind === 'src') {
      // An image: a blob ref (every new image), or an older data: image (still shown, never made again).
      if (v == null || v === '') return ''
      if (isBlobRef(v)) return v
      if (typeof v === 'string' && v.startsWith('data:image/') && v.length > 400000) throw refusal('too_long', key, 400000)
      if (isDataImage(v)) return v
      bad('an image ref (k2blob:sha256-…)')
    }
    // The CLI may pass JSON as text.
    if (typeof v === 'string' && /^[\[{]/.test(v)) { try { v = JSON.parse(v) } catch (_) { bad('valid JSON') } }
    if (kind === 'list') {
      if (typeof v === 'string') v = v.split(',').map((s) => s.trim()).filter(Boolean)
      if (!Array.isArray(v)) bad('a list')
      if (v.length > LIMITS.tags) throw refusal('too_many', key, LIMITS.tags)
      return v.map((s) => (typeof s === 'number' && Number.isFinite(s) ? s : strOf(s, key, 80)))
    }
    if (kind === 'points') {
      if (!Array.isArray(v) || v.some((p) => !Array.isArray(p) || p.length < 2)) bad('a list of [x, y]')
      if (v.length > LIMITS.points) throw refusal('too_many', key, LIMITS.points)
      return v.map((p) => [Math.round(numOf(p[0], key)), Math.round(numOf(p[1], key))])
    }
    if (kind === 'rows') {
      if (!Array.isArray(v) || v.some((r) => !Array.isArray(r))) bad('a list of rows')
      if (v.length > LIMITS.rows) throw refusal('too_many', key, LIMITS.rows)
      if (v.some((r) => r.length > LIMITS.cols)) throw refusal('too_many', key, LIMITS.cols)
      return v.map((r) => r.map((c) => strOf(c, key, 2000)))
    }
    if (kind === 'rowIds' || kind === 'colIds') {
      if (!Array.isArray(v) || v.some((x) => typeof x !== 'string' || !ITEM_ID[kind].test(x))) bad('a list of ' + (kind === 'rowIds' ? 'rw-' : 'cl-') + '… ids')
      if (new Set(v).size !== v.length) bad('ids used twice')
      if (v.length > (kind === 'rowIds' ? LIMITS.rows : LIMITS.cols)) throw refusal('too_many', key, kind === 'rowIds' ? LIMITS.rows : LIMITS.cols)
      return v.slice()
    }
    if (kind === 'end') {
      if (typeof v === 'string') { if (chars(v) > 40) throw refusal('too_long', key, 40); return { id: v, anchor: 'auto' } }
      if (!isObj(v)) bad('an object id, {"id":..,"anchor":..} or {"x":..,"y":..}')
      if (v.id != null && v.id !== '') {
        if (typeof v.id !== 'string') bad('an object id')
        if (chars(v.id) > 40) throw refusal('too_long', key, 40)
        if (v.anchor != null && !ANCHORS.includes(v.anchor)) bad('anchor: one of ' + ANCHORS.join(', '))
        const e = { id: v.id, anchor: v.anchor || 'auto' }
        // Where the end was last drawn (optional): shown faded if the object it names is gone.
        if (v.x != null && v.y != null) { e.x = check('num', v.x, key + '.x'); e.y = check('num', v.y, key + '.y') }
        return e
      }
      return { x: check('num', v.x, key + '.x'), y: check('num', v.y, key + '.y') }
    }
    if (kind === 'checks') {
      if (v == null) return null
      if (!Array.isArray(v)) bad('a list of {"text":..,"done":true|false}')
      if (v.length > LIMITS.checks) throw refusal('too_many', key, LIMITS.checks)
      return v.map((c, i) => {
        if (!isObj(c)) return { id: 'ck-' + i, text: strOf(c, key, 500), done: false }
        if (c.id != null && (typeof c.id !== 'string' || !ITEM_ID.checks.test(c.id))) bad('item ids are ck-…')
        return { id: c.id || 'ck-' + i, text: strOf(c.text, key, 500), done: c.done === true || c.done === 'true' }
      })
    }
    if (kind === 'replies') {
      if (v == null) return []
      if (!Array.isArray(v)) bad('a list of {"by":..,"at":..,"text":..}')
      return v.map((r, i) => {
        if (!isObj(r)) bad('a list of {"by":..,"at":..,"text":..}')
        if (r.id != null && (typeof r.id !== 'string' || !ITEM_ID.replies.test(r.id))) bad('reply ids are rp-…')
        return { id: r.id || 'rp-' + i, by: strOf(r.by, key, 80), at: strOf(r.at, key, 40), text: strOf(r.text, key, 5000) }
      })
    }
    if (kind === 'reactions') {
      // {"<person>": ["👍", …]}. The older shape {"👍": ["<name>", …]} is turned round on read.
      if (v == null) return {}
      if (!isObj(v)) bad('{"<person>": ["👍", …]}')
      let byPerson = v
      if (Object.keys(v).some((k) => REACTIONS.includes(k))) {
        byPerson = {}
        for (const em of Object.keys(v)) {
          if (!REACTIONS.includes(em)) bad('one of ' + REACTIONS.join(' '))
          if (!Array.isArray(v[em])) bad('{"<person>": ["👍", …]}')
          for (const who of v[em]) { const k = strOf(who, key, 80); if (!k) continue; (byPerson[k] = byPerson[k] || []).push(em) }
        }
      }
      const out = {}
      for (const who of Object.keys(byPerson)) {
        if (chars(who) > 80) throw refusal('too_long', key, 80)
        const list = byPerson[who]
        if (!Array.isArray(list) || list.some((em) => !REACTIONS.includes(em))) bad('{"<person>": ["👍", …]} with ' + REACTIONS.join(' '))
        const uniq = []; for (const em of list) if (!uniq.includes(em)) uniq.push(em)
        if (uniq.length) out[who] = uniq
      }
      return out
    }
    if (kind === 'votes') {
      if (v == null) return {}
      if (!isObj(v)) bad('{"<person>": count}')
      const out = {}
      for (const who of Object.keys(v)) {
        if (chars(who) > 80) throw refusal('too_long', key, 80)
        const n = v[who]
        if (!Number.isInteger(n) || n < 0) bad('a whole number of votes')
        if (n > 0) out[who] = n
      }
      return out
    }
    if (kind === 'crop') {
      if (v == null) return null
      if (!isObj(v)) bad('{"x":..,"y":..,"w":..,"h":..}, fractions of the image from 0 to 1')
      const n = (q) => { const x = numOf(v[q], key); if (x < 0 || x > 1) bad('crop.' + q + ' from 0 to 1'); return Math.round(x * 10000) / 10000 }
      const c = { x: n('x'), y: n('y'), w: n('w'), h: n('h') }
      if (c.w < 0.01 || c.h < 0.01 || c.x + c.w > 1.0001 || c.y + c.h > 1.0001) bad('a crop inside the image')
      return c
    }
    if (kind === 'json') {
      const s = JSON.stringify(v)
      if (s === undefined) bad('JSON')
      if (s.length > 20000) throw refusal('value_too_large', key, 20000)
      return JSON.parse(s)
    }
    bad('unknown kind ' + kind)
  }

  // Validate fields for a type. An unknown type or field is refused (fields the build doesn't know travel
  // separately, through splitExtra). A table's rows and their ids must stay in step.
  function clean(type, f) {
    if (!TYPES[type]) throw refusal('unknown_type', 'type')
    const spec = Object.assign({}, COMMON, TYPES[type])
    const out = {}
    for (const k of Object.keys(f)) {
      if (k === 'id' || k === 'type') continue
      if (!spec[k]) throw refusal('unknown_field', k)
      out[k] = check(spec[k], f[k], k)
    }
    if (out.rows && out.rowIds && out.rowIds.length !== out.rows.length) throw refusal('bad_field', 'rowIds', null, 'one id per row')
    if (out.rows && out.colIds && out.colIds.length !== colCount(out.rows)) throw refusal('bad_field', 'colIds', null, 'one id per column')
    return out
  }
  const colCount = (rows) => rows.reduce((n, r) => Math.max(n, r.length), 0)
  // An object as read: list items without ids get them by position (ck-0, rp-0, rw-0, cl-0). They are
  // stored only when the object is next written.
  function ensureIds(o) {
    if (!o) return o
    if (o.type === 'table' && Array.isArray(o.rows)) {
      if (!Array.isArray(o.rowIds) || o.rowIds.length !== o.rows.length) o.rowIds = o.rows.map((_, i) => 'rw-' + i)
      const n = colCount(o.rows)
      if (!Array.isArray(o.colIds) || o.colIds.length !== n) o.colIds = Array.from({ length: n }, (_, i) => 'cl-' + i)
    }
    return o
  }
  // A whole object's stored size, as K2 counts it (UTF-8 JSON).
  const bytesOf = (v) => new TextEncoder().encode(JSON.stringify(v)).length
  function checkSize(v) { if (bytesOf(v) > VALUE_MAX) throw refusal('value_too_large', null, VALUE_MAX) }

  // Fill in what can be derived: a pen's box from its points. When points are given, the box
  // is their min/max and the points are shifted to start at 0 (moving x/y to match).
  function normalize(type, f, base) {
    if (type !== 'pen' || f.points == null) return f
    let pts = f.points
    if (typeof pts === 'string') { try { pts = JSON.parse(pts) } catch (_) { return f } }
    if (!Array.isArray(pts) || !pts.length || pts.some((p) => !Array.isArray(p))) return f
    let x1 = Infinity; let y1 = Infinity; let x2 = -Infinity; let y2 = -Infinity
    for (const p of pts) { x1 = Math.min(x1, +p[0]); y1 = Math.min(y1, +p[1]); x2 = Math.max(x2, +p[0]); y2 = Math.max(y2, +p[1]) }
    if (![x1, y1, x2, y2].every(Number.isFinite)) return f
    const out = Object.assign({}, f, { points: pts.map((p) => [Math.round(p[0] - x1), Math.round(p[1] - y1)]), w: Math.max(1, Math.round(x2 - x1)), h: Math.max(1, Math.round(y2 - y1)) })
    if (x1 || y1) { const b = base || {}; out.x = (f.x != null ? +f.x : (b.x || 0)) + x1; out.y = (f.y != null ? +f.y : (b.y || 0)) + y1 }
    return out
  }
  // What a board setting is when it was never set (so undo can send a real value).
  const META_OFF = { voting: { active: false }, timer: { state: 'off' }, title: '' }
  // Fields this build doesn't know (a newer build's, or another tool's): kept as they are, never dropped.
  // A plain lowercase-first name that isn't one of ours or bookkeeping. Old readers keep new fields this way.
  const RESERVED = ['id', 'type', 'batch', 'createdBy', 'createdAt', 'updatedBy', 'updatedAt']
  // A name that is any type's field (rows, replies, title …) is never an extra: on the wrong type it would mislead code that reads it.
  const ANY_FIELD = new Set(Object.values(TYPES).flatMap((t) => Object.keys(t)))
  // Fields where null means "none": the key is removed rather than kept as null.
  const NULL_IS_NONE = ['checks', 'crop', 'link']
  const dropsOnNull = (type, k) => NULL_IS_NONE.includes(k) || isExtraKey(type, k)
  function isExtraKey(type, k) { return /^[a-z][A-Za-z0-9_]{0,39}$/.test(k) && !RESERVED.includes(k) && !(k in COMMON) && !ANY_FIELD.has(k) }
  function splitExtra(type, f) {
    const known = {}; const extra = {}
    for (const k of Object.keys(f || {})) {
      if (!isExtraKey(type, k)) { known[k] = f[k]; continue }
      const j = JSON.stringify(f[k])
      if (j === undefined) throw refusal('bad_field', k, null, 'JSON')
      extra[k] = JSON.parse(j)
    }
    return { known, extra }
  }
  // Board settings: known ones are checked; any other plain name is kept as it came.
  function cleanMeta(f) {
    const out = {}
    for (const k of Object.keys(f)) {
      if (META[k]) { out[k] = check(META[k], f[k], k); continue }
      if (!/^[a-z][A-Za-z0-9_]{0,39}$/.test(k) || k === 'batch') throw refusal('unknown_field', k)
      out[k] = JSON.parse(JSON.stringify(f[k]))
    }
    return out
  }

  // ---------- templates: each returns ops at origin (x, y) ----------
  function templates(newId) {
    const add = (type, f) => ({ op: 'add', obj: Object.assign({ id: newId(PREFIX[type]), type }, f) })
    const frame = (x, y, w, h, title, fill) => add('frame', { x, y, w, h, title, fill: fill || 'white' })
    const sticky = (x, y, text, color) => add('sticky', { x, y, w: 160, h: 160, text, color })
    const text = (x, y, w, t, size, bold) => add('text', { x, y, w, h: size * 1.6, text: t, fontSize: size, bold: !!bold })
    const T = {}

    const hint = (x, y, w, t) => add('text', { x, y, w, h: 24, text: t, fontSize: 14, color: 'gray' })
    const link = (p, q, label, fa, ta, route) => add('connector', { from: { id: p.obj.id, anchor: fa || 'auto' }, to: { id: q.obj.id, anchor: ta || 'auto' }, route: route || 'elbow', endCap: 'arrow', label: label || '' })

    // Kanban: tinted columns with a hint, and example cards that show every field (owner, due date, status).
    T.kanban = (x, y) => {
      const cols = [['To do', 'gray', 'What’s next'], ['In progress', 'blue', 'Being worked on'], ['Review', 'violet', 'Waiting for a check'], ['Done', 'green', 'Finished this week']]
      const W = 4 * 320 + 40
      const ops = [frame(x, y, W, 760, 'Kanban'), text(x + 24, y + 20, 600, 'Kanban board', 32, true), hint(x + 26, y + 70, 900, 'Drag cards between columns. Double-click a card to edit it; its Fields button sets owner, due date and status.')]
      cols.forEach(([name, c, sub], i) => {
        const cx = x + 24 + i * 320
        ops.push(add('shape', { x: cx, y: y + 110, w: 296, h: 620, shape: 'round', fill: c === 'gray' ? 'gray' : c, stroke: 'gray', strokeWidth: 0, text: '' }))
        ops.push(text(cx + 16, y + 122, 260, name, 20, true))
        ops.push(hint(cx + 17, y + 156, 260, sub))
        const first = { title: name === 'Done' ? 'Kickoff notes shared' : name === 'Review' ? 'Draft the brief' : name === 'In progress' ? 'Interview three customers' : 'Plan the next sprint', text: 'Describe the work', color: c, status: name, assignee: i % 2 ? 'Teammate' : 'You', due: '' }
        ops.push(add('card', Object.assign({ x: cx + 8, y: y + 196, w: 280, h: 140 }, first)))
        if (i < 2) ops.push(add('card', { x: cx + 8, y: y + 352, w: 280, h: 140, title: i ? 'Fix the onboarding copy' : 'Pick a launch date', text: '', color: c, status: name, assignee: '', due: '' }))
      })
      return ops
    }
    // Retrospective: colour-coded columns with the questions, two notes each, and how to vote.
    T.retro = (x, y) => {
      const cols = [['Went well', 'green', 'What should we keep doing?'], ['To improve', 'pink', 'What slowed us down?'], ['Action items', 'blue', 'What will we change, and who owns it?']]
      const ops = [frame(x, y, 3 * 400 + 40, 800, 'Retrospective'), text(x + 24, y + 20, 800, 'Retrospective', 32, true), hint(x + 26, y + 70, 1000, 'Everyone adds notes for 5 minutes (the timer is top right), then vote on the ones to act on.')]
      cols.forEach(([name, c, q], i) => {
        const cx = x + 24 + i * 400
        ops.push(add('shape', { x: cx, y: y + 110, w: 380, h: 660, shape: 'round', fill: 'gray', stroke: 'gray', strokeWidth: 0 }))
        ops.push(add('shape', { x: cx, y: y + 110, w: 380, h: 60, shape: 'round', fill: c, stroke: c, strokeWidth: 0, text: name, fontSize: 20 }))
        ops.push(hint(cx + 20, y + 182, 340, q))
        ops.push(sticky(cx + 20, y + 222, 'Add a note', c === 'blue' ? 'cyan' : c))
        ops.push(sticky(cx + 200, y + 222, '', c === 'blue' ? 'cyan' : c))
      })
      return ops
    }
    T.flowchart = (x, y) => {
      const head = [text(x + 24, y + 20, 600, 'Flowchart', 32, true), hint(x + 26, y + 70, 860, 'Drag from a shape’s edge to draw an arrow; arrows stay attached when shapes move.')]
      const a = add('shape', { x: x + 330, y: y + 130, w: 180, h: 70, shape: 'flow-start', text: 'Start', fill: 'green' })
      const b = add('shape', { x: x + 320, y: y + 260, w: 200, h: 90, shape: 'flow-process', text: 'Do the thing', fill: 'blue' })
      const c = add('shape', { x: x + 320, y: y + 410, w: 200, h: 130, shape: 'flow-decision', text: 'Worked?', fill: 'yellow' })
      const d = add('shape', { x: x + 330, y: y + 610, w: 180, h: 70, shape: 'flow-start', text: 'End', fill: 'red' })
      const e = add('shape', { x: x + 620, y: y + 430, w: 200, h: 90, shape: 'flow-process', text: 'Fix it', fill: 'orange' })
      return [frame(x, y, 920, 740, 'Flowchart'), ...head, a, b, c, d, e,
        link(a, b), link(b, c), link(c, d, 'Yes', 'bottom', 'top'), link(c, e, 'No', 'right', 'left'), link(e, b, '', 'top', 'right')]
    }
    T.storymap = (x, y) => {
      const acts = ['Discover', 'Sign up', 'Use', 'Share']
      const ops = [frame(x, y, acts.length * 240 + 80, 820, 'User story map'), text(x + 24, y + 20, 600, 'User story map', 32, true)]
      acts.forEach((name, i) => {
        const cx = x + 40 + i * 240
        ops.push(add('sticky', { x: cx, y: y + 100, w: 200, h: 110, text: name, color: 'orange' }))
        ops.push(add('sticky', { x: cx, y: y + 240, w: 200, h: 110, text: 'Step', color: 'yellow' }))
        ops.push(add('sticky', { x: cx, y: y + 380, w: 200, h: 110, text: 'Story (release 1)', color: 'blue' }))
        ops.push(add('sticky', { x: cx, y: y + 600, w: 200, h: 110, text: 'Story (release 2)', color: 'violet' }))
      })
      ops.push(add('shape', { x: x + 24, y: y + 560, w: acts.length * 240 + 32, h: 4, shape: 'rect', fill: 'gray', stroke: 'gray', strokeWidth: 0 }))
      ops.push(text(x + 40, y + 520, 300, 'Release 2', 16, true))
      return ops
    }
    T.brainstorm = (x, y) => {
      const ops = [frame(x, y, 1000, 700, 'Brainstorm')]
      ops.push(add('shape', { x: x + 380, y: y + 280, w: 240, h: 140, shape: 'ellipse', text: 'Big question?', fill: 'violet', fontSize: 22 }))
      const colors = ['yellow', 'pink', 'blue', 'green', 'orange', 'cyan']
      for (let i = 0; i < 6; i++) {
        const a = (i / 6) * Math.PI * 2 - Math.PI / 2
        ops.push(sticky(x + 420 + Math.cos(a) * 340, y + 270 + Math.sin(a) * 230, '', colors[i]))
      }
      return ops
    }
    T.mindmap = (x, y) => {
      const root = add('mind', { x: x + 400, y: y + 260, w: 200, h: 60, text: 'Central idea', color: 'violet' })
      const ops = [frame(x, y, 1000, 580, 'Mind map'), root]
      const kids = [['Goals', -1, -1], ['People', 1, -1], ['Risks', -1, 1], ['Timeline', 1, 1]]
      for (const [t, sx, sy] of kids) ops.push(add('mind', { x: x + 400 + sx * 300, y: y + 260 + sy * 110, w: 160, h: 48, text: t, parent: root.obj.id, color: sx < 0 ? 'blue' : 'green' }))
      return ops
    }
    // Project kickoff: goals, who does what, the milestones in order, risks, open questions, and first steps.
    T.kickoff = (x, y) => {
      const ops = [frame(x, y, 1560, 1020, 'Project kickoff'), text(x + 24, y + 20, 900, 'Project kickoff', 32, true),
        hint(x + 26, y + 70, 1200, 'Fill each part together in the first meeting; the agents with access can read it and help.')]
      const section = (sx, sy, w, h, title, c) => { ops.push(add('shape', { x: sx, y: sy, w, h, shape: 'round', fill: 'gray', stroke: 'gray', strokeWidth: 0 })); ops.push(text(sx + 18, sy + 12, w - 36, title, 20, true)) }
      // Goals
      section(x + 24, y + 110, 480, 300, 'Goals', 'gray')
      ;['What does success look like?', 'Who is it for?', 'What is out of scope?'].forEach((t, i) => ops.push(add('sticky', { x: x + 40 + i * 154, y: y + 160, w: 142, h: 142, text: t, color: ['yellow', 'green', 'pink'][i] })))
      // Team and roles
      section(x + 528, y + 110, 500, 300, 'Team and roles', 'gray')
      ;[['Owner', 'Decides and unblocks', 'violet'], ['Builder', 'Does the work', 'blue'], ['Reviewer', 'Checks before it ships', 'green']].forEach(([r, d, c], i) => ops.push(add('card', { x: x + 544, y: y + 156 + i * 80, w: 468, h: 70, title: r, text: d, color: c, assignee: '', status: '' })))
      // Risks
      section(x + 1052, y + 110, 484, 300, 'Risks', 'gray')
      ;['What could go wrong?', 'What do we depend on?'].forEach((t, i) => ops.push(add('sticky', { x: x + 1068 + i * 230, y: y + 160, w: 210, h: 140, text: t, color: 'orange' })))
      // Timeline: milestones in order
      section(x + 24, y + 434, 1512, 220, 'Milestones', 'gray')
      const ms = ['Kickoff', 'Plan agreed', 'First version', 'Review', 'Launch']
      const shapes = ms.map((t, i) => add('shape', { x: x + 56 + i * 300, y: y + 500, w: 220, h: 110, shape: i === ms.length - 1 ? 'star' : 'round', text: t, fill: i === ms.length - 1 ? 'yellow' : 'white', fontSize: 18 }))
      ops.push(...shapes)
      for (let i = 1; i < shapes.length; i++) ops.push(link(shapes[i - 1], shapes[i], '', 'right', 'left', 'straight'))
      // Open questions, and first steps as a small board
      section(x + 24, y + 678, 600, 316, 'Open questions', 'gray')
      ops.push(add('sticky', { x: x + 40, y: y + 728, w: 180, h: 140, text: 'What do we still need to decide?', color: 'violet' }))
      section(x + 648, y + 678, 888, 316, 'First steps', 'gray')
      ;[['To do', 'gray'], ['Doing', 'blue'], ['Done', 'green']].forEach(([t, c], i) => {
        ops.push(text(x + 668 + i * 290, y + 722, 200, t, 16, true))
        ops.push(add('card', { x: x + 664 + i * 290, y: y + 756, w: 270, h: 110, title: i === 0 ? 'Book the first review' : i === 1 ? 'Write the plan' : 'Kickoff held', text: '', color: c, status: t, assignee: '' }))
      })
      return ops
    }
    return T
  }
  const TEMPLATE_NAMES = { kickoff: 'Project kickoff', kanban: 'Kanban', retro: 'Retrospective', flowchart: 'Flowchart', storymap: 'User story map', brainstorm: 'Brainstorm', mindmap: 'Mind map' }


  g.BoardSchema = { FORMAT, STICKY, INK, FILL, SHAPES, CAPS, ROUTES, DASH, ANCHORS, TYPES, COMMON, DEFAULTS, PREFIX, META, REACTIONS, STAMPS, LIMITS, VALUE_MAX,
    clean, cleanMeta, check, normalize, META_OFF, templates, TEMPLATE_NAMES, splitExtra, isExtraKey, dropsOnNull,
    refusal, isRefusal, ensureIds, itemId, isBlobRef, isDataImage, bytesOf, checkSize, chars }
})(typeof window !== 'undefined' ? window : globalThis)
