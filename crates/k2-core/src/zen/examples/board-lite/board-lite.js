// Board Lite: sticky notes kept in this widget's own data (k2.store), the
// format Board uses (k2 zen guide services):
//   - one key per object: <prefix>obj/<id> = {id, text, x, y, color, batch}
//     (`prefix` is the placement's config.prefix, e.g. project/7/board/1/);
//   - field-level ops: a gesture changes some fields; the write carries the
//     rev it read (ifRev), and on `conflict` it reads the note again and
//     re-applies only the fields it changed;
//   - batch ids: every write names the gesture that made it;
//   - live: k2.store.subscribe shows anyone's change (yours, another window,
//     an agent's `k2 store put --widget board-lite …`) with no reload.
// Text an agent wrote always goes in with textContent, never innerHTML.
(function () {
  const board = document.getElementById('board')
  const count = document.getElementById('count')
  const add = document.getElementById('add')
  const banner = document.getElementById('note')
  const COLORS = ['yellow', 'blue', 'green', 'pink']

  let prefix = ''
  let viewerName = 'You'
  let canWrite = false
  /** key → {rev, value, by, el} */
  const notes = new Map()

  const objPrefix = () => prefix + 'obj/'
  const newId = () => Date.now().toString(36) + Math.random().toString(36).slice(2, 7)

  function show(text) {
    banner.textContent = text
    banner.hidden = !text
  }

  function report(err) {
    const code = err && err.code
    if (code === 'author_unknown') {
      show('Nobody signed this version of the widget, so it gets no data. Run k2 zen validate --widget board-lite, or click It’s mine in Settings → Gardens.')
    } else if (code === 'not_granted') {
      show('This widget’s maker has no grant for this data. Settings → Gardens → Widget data: Let … use this data, or It’s mine.')
    } else if (code === 'cap_not_granted') {
      show('This widget can’t save here (store:write is off).')
    } else {
      show((err && err.message) || String(err))
    }
  }

  function label(by) {
    if (!by) return ''
    if (by === 'you') return viewerName
    if (by.indexOf('you via ') === 0) return viewerName + ' (via ' + by.slice(8) + ')'
    if (by.indexOf('agent:') === 0) return by.slice(6)
    return by
  }

  function draw(key) {
    const n = notes.get(key)
    if (!n) return
    const v = n.value || {}
    if (!n.el) {
      n.el = document.createElement('div')
      n.el.className = 'sticky'
      n.el.dataset.key = key
      const text = document.createElement('div')
      text.className = 'text'
      const by = document.createElement('span')
      by.className = 'by'
      const rm = document.createElement('button')
      rm.type = 'button'
      rm.className = 'rm'
      rm.textContent = '×'
      rm.setAttribute('aria-label', 'Remove note')
      rm.addEventListener('click', (e) => {
        e.stopPropagation()
        remove(key)
      })
      n.el.append(text, by, rm)
      n.el.addEventListener('pointerdown', (e) => startDrag(key, e))
      n.el.addEventListener('dblclick', () => edit(key))
      board.append(n.el)
    }
    n.el.style.left = (Number(v.x) || 0) + 'px'
    n.el.style.top = (Number(v.y) || 0) + 'px'
    n.el.dataset.color = COLORS.indexOf(v.color) >= 0 ? v.color : 'yellow'
    // Values come from anyone with a grant: tolerate a bad shape.
    n.el.querySelector('.text').textContent = typeof v.text === 'string' ? v.text : ''
    n.el.querySelector('.by').textContent = label(n.by)
  }

  function forget(key) {
    const n = notes.get(key)
    if (n && n.el) n.el.remove()
    notes.delete(key)
  }

  function recount() {
    count.textContent = notes.size + (notes.size === 1 ? ' note' : ' notes')
  }

  /** One change from the feed (or a list row). `value` null = deleted. */
  function apply(row) {
    if (row.key.indexOf(objPrefix()) !== 0) return
    const have = notes.get(row.key)
    if (have && have.rev > row.rev) return
    if (row.value === null || row.value === undefined || row.deleted) {
      forget(row.key)
    } else {
      notes.set(row.key, { rev: row.rev, value: row.value, by: row.by, el: have ? have.el : null })
      draw(row.key)
    }
  }

  /** Write `fields` onto the note at `key` (field-level): ifRev = the rev we
   *  read; on conflict read it again and re-apply only these fields. */
  async function change(key, fields) {
    const batch = newId()
    for (let attempt = 0; attempt < 3; attempt++) {
      const have = notes.get(key)
      const base = have ? have.value : {}
      const value = Object.assign({}, base, fields, { batch: batch })
      try {
        const r = await k2.store.put(key, value, { ifRev: have ? have.rev : 0 })
        apply({ key: key, rev: r.rev, value: value, by: 'you' })
        show('')
        return
      } catch (err) {
        if (!err || err.code !== 'conflict') throw err
        const now = await k2.store.get(key)
        if (now) apply(now)
        else forget(key)
      }
    }
    throw new Error('Someone else keeps changing this note; try again.')
  }

  async function remove(key) {
    if (!canWrite) return
    const have = notes.get(key)
    try {
      await k2.store.delete(key, { ifRev: have ? have.rev : undefined })
      forget(key)
      recount()
    } catch (err) {
      report(err)
    }
  }

  function startDrag(key, e) {
    if (!canWrite || e.target.closest('button, textarea')) return
    const n = notes.get(key)
    if (!n) return
    const el = n.el
    const sx = e.clientX
    const sy = e.clientY
    const ox = Number(n.value.x) || 0
    const oy = Number(n.value.y) || 0
    el.classList.add('dragging')
    if (el.setPointerCapture) el.setPointerCapture(e.pointerId)
    const move = (m) => {
      el.style.left = Math.max(0, ox + m.clientX - sx) + 'px'
      el.style.top = Math.max(0, oy + m.clientY - sy) + 'px'
    }
    const up = (u) => {
      el.removeEventListener('pointermove', move)
      el.removeEventListener('pointerup', up)
      el.classList.remove('dragging')
      const x = Math.max(0, Math.round(ox + u.clientX - sx))
      const y = Math.max(0, Math.round(oy + u.clientY - sy))
      if (x !== ox || y !== oy) change(key, { x: x, y: y }).catch(report)
    }
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerup', up)
  }

  function edit(key) {
    const n = notes.get(key)
    if (!canWrite || !n || n.el.querySelector('textarea')) return
    const text = n.el.querySelector('.text')
    const box = document.createElement('textarea')
    box.maxLength = 2000
    box.value = typeof n.value.text === 'string' ? n.value.text : ''
    text.hidden = true
    n.el.insertBefore(box, text)
    box.focus()
    const done = () => {
      const next = box.value
      box.remove()
      text.hidden = false
      if (next !== n.value.text) change(key, { text: next }).catch(report)
    }
    box.addEventListener('blur', done, { once: true })
    box.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) box.blur()
    })
  }

  add.addEventListener('click', () => {
    const id = newId()
    const at = notes.size * 18
    change(objPrefix() + id, {
      id: id,
      text: 'New note',
      x: 12 + (at % 240),
      y: 12 + (at % 160),
      color: COLORS[notes.size % COLORS.length],
    })
      .then(recount)
      .catch(report)
  })

  async function loadAll() {
    const seen = new Set()
    let after
    do {
      const page = await k2.store.list({ prefix: objPrefix(), after: after, limit: 500 })
      for (const row of page.items) {
        seen.add(row.key)
        apply(row)
      }
      after = page.next || undefined
    } while (after)
    for (const key of Array.from(notes.keys())) if (!seen.has(key)) forget(key)
    recount()
  }

  // K2 connects the frame after this script runs: wait before k2.can and
  // k2.config. Calls wait on their own.
  k2.connected.then(
    async () => {
      prefix = typeof (k2.config && k2.config.prefix) === 'string' ? k2.config.prefix : ''
      canWrite = k2.can('store.put')
      add.disabled = !canWrite
      try {
        const me = await k2.viewer()
        if (me && me.name) viewerName = me.name
      } catch (err) {
        // The name is only a label; keep "You".
      }
      try {
        await loadAll()
        k2.store.subscribe(
          objPrefix(),
          (changes) => {
            if (changes && changes.resync) {
              for (const key of Array.from(notes.keys())) forget(key)
              for (const row of changes.items) apply(row)
            } else {
              for (const row of changes) apply(row)
            }
            recount()
          },
          report,
        )
      } catch (err) {
        report(err)
      }
      k2.ready()
    },
    () => show('K2 didn’t start this widget.'),
  )
})()
