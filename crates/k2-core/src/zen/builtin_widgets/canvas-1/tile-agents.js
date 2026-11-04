// The Agents tile (k2:canvas@1): every agent this Garden reaches (this
// computer's and the ones on your Homes), in K2's order, each with its
// picture, whether it's working or needs you, and your unread count. A
// click shows that agent in the Thread beside the canvas.
//
// K2's own verbs only:
//   k2.agents.subscribe   the rows, live (agents:read)
//   k2.thread.unread      your unread and waiting cards per Thread on this
//                         computer (thread:read), re-read when an agent's
//                         last message changes and every 30 s
//   k2.conversation.open  show the pick in the Conversation that follows
//                         this widget (the Garden's Thread column)
//   k2.layout.*           unfold that column on a pick (right after the click)
// Every value from an agent goes in with textContent; a picture is only
// ever a PNG, JPEG, WebP or GIF data: URL in an <img>.
(function () {
  'use strict'
  var k2 = window.k2
  var UNREAD_MS = 30000
  var SEARCH_FROM = 8
  var IMAGE = /^data:image\/(png|jpeg|webp|gif);base64,[A-Za-z0-9+/=]+$/

  function el(tag, cls, text) {
    var e = document.createElement(tag)
    if (cls) e.className = cls
    if (text != null) e.textContent = String(text)
    return e
  }
  function initials(label) {
    var w = String(label || '?').split(/[\s_-]+/).filter(Boolean)
    return ((w[0] || '?')[0] + (w[1] ? w[1][0] : '')).toUpperCase()
  }
  function handleOf(address) {
    return String(address || '').split('::')[0]
  }
  function words(err) {
    return String((err && err.code) || 'failed').replace(/_/g, ' ')
  }

  // What a row's status says, in a word, and its dot.
  function status(r) {
    if (r.state && r.state !== 'ok') return { text: r.stateLabel || String(r.state).replace(/-/g, ' '), dot: 'ag-off' }
    if (r.needsYou || r.activity === 'needs-you') return { text: 'needs you', dot: 'ag-needs' }
    if (r.working || r.activity === 'working' || r.activity === 'monitoring') return { text: 'working', dot: 'ag-working' }
    return { text: '', dot: '' }
  }

  window.K2Canvas.tile({
    id: 'agents',
    name: 'Agents',
    description: 'Every agent you can reach, with its picture, whether it’s working, and your unread messages. Pick one to talk in the Thread.',
    size: 'half',
    needs: ['agents.subscribe', 'conversation.open'],
    mount: function (ctx) {
      var rows = []
      var unread = {}
      var unreadOk = false
      var seqs = ''
      var filter = ''
      var column = null
      var stopped = false
      var off = null
      var offLayout = null
      var timer = 0

      var root = el('div', 'ag')
      var search = el('input', 'ag-search')
      search.type = 'search'
      search.placeholder = 'Find an agent'
      search.setAttribute('aria-label', 'Find an agent')
      search.hidden = true
      root.appendChild(search)
      var listEl = el('ul', 'ag-list')
      listEl.setAttribute('aria-label', 'Agents')
      root.appendChild(listEl)
      ctx.body.appendChild(root)

      function countFor(r) {
        if (r.server != null) return null
        var u = unread[handleOf(r.address)]
        return u ? u.unread + u.waiting : 0
      }

      function draw() {
        var shown = rows.slice().sort(function (a, b) { return (a.index || 0) - (b.index || 0) })
        search.hidden = rows.length <= SEARCH_FROM
        var q = filter.trim().toLowerCase()
        if (q) {
          shown = shown.filter(function (r) {
            return String(r.label || '').toLowerCase().indexOf(q) >= 0 || String(r.address || '').toLowerCase().indexOf(q) >= 0
          })
        }
        listEl.textContent = ''
        var working = 0, needs = 0, total = 0
        rows.forEach(function (r) {
          var s = status(r)
          if (s.dot === 'ag-working') working++
          if (s.dot === 'ag-needs') needs++
          total += countFor(r) || 0
        })
        var bits = [rows.length + (rows.length === 1 ? ' agent' : ' agents')]
        if (working) bits.push(working + ' working')
        if (needs) bits.push(needs + ' need' + (needs === 1 ? 's' : '') + ' you')
        if (unreadOk && total) bits.push(total + ' unread')
        ctx.setMeta(rows.length ? bits.join(' · ') : '')
        if (!rows.length) {
          listEl.appendChild(el('li', 'ag-empty', 'No agents here yet.'))
          return
        }
        if (!shown.length) {
          listEl.appendChild(el('li', 'ag-empty', 'No agent matches “' + filter.trim() + '”.'))
          return
        }
        shown.forEach(function (r) {
          var li = el('li', 'ag-li')
          var b = el('button', 'ag-row')
          b.type = 'button'
          if (r.selected) {
            b.classList.add('ag-sel')
            b.setAttribute('aria-current', 'true')
          }
          b.disabled = r.openable === false
          var face = el('span', 'ag-face')
          if (typeof r.image === 'string' && IMAGE.test(r.image)) {
            var img = el('img', 'ag-img')
            img.alt = ''
            img.src = r.image
            face.appendChild(img)
          } else {
            face.textContent = initials(r.label)
            face.style.backgroundColor = /^#[0-9a-f]{6}$/i.test(r.color || '') ? r.color : ''
          }
          var s = status(r)
          var dot = el('span', 'ag-dot ' + s.dot)
          face.appendChild(dot)
          b.appendChild(face)
          var label = el('span', 'ag-words')
          label.appendChild(el('span', 'ag-name', r.label || r.address))
          var sub = []
          if (s.text) sub.push(s.text)
          if (r.server) sub.push(r.server)
          if (sub.length) label.appendChild(el('span', 'ag-sub ' + (s.dot || ''), sub.join(' · ')))
          b.appendChild(label)
          var n = countFor(r)
          if (n) {
            var badge = el('span', 'ag-badge', n > 99 ? '99+' : n)
            badge.title = n + ' unread'
            b.appendChild(badge)
          }
          var title = (r.label || r.address) + (r.server ? ' on ' + r.server : ' on this computer')
          if (s.text) title += ' · ' + s.text
          if (n) title += ' · ' + n + ' unread'
          if (r.detail) title += '\n' + r.detail
          b.title = b.disabled ? title + '\n(can’t be opened right now)' : title + '\nShow in the Thread'
          b.addEventListener('click', function () { pick(r) })
          li.appendChild(b)
          listEl.appendChild(li)
        })
      }

      // The Thread column beside the canvas: the collapsible one labelled
      // "Thread", else the first collapsible one.
      function threadColumn(info) {
        var cols = info && Array.isArray(info.columns) ? info.columns : []
        var named = cols.filter(function (c) { return c && c.collapsible && c.label === 'Thread' })[0]
        return named || cols.filter(function (c) { return c && c.collapsible })[0] || null
      }

      function pick(r) {
        if (!r.address) return
        // Right after the click: unfold the Thread if it is folded.
        if (column && column.collapsed && ctx.can('layout.set')) {
          k2.layout.set({ column: column.index, collapsed: false }).catch(function () { /* the person unfolds it */ })
        }
        Promise.resolve().then(function () { return k2.conversation.open(r.address) }).catch(function (err) {
          ctx.toast('K2 didn’t open ' + (r.label || r.address) + ' (' + words(err) + ')')
        })
      }

      function readUnread() {
        clearTimeout(timer)
        if (stopped || !ctx.can('thread.unread')) return
        Promise.resolve().then(function () { return k2.thread.unread() }).then(function (u) {
          var next = {}
          ;(u && Array.isArray(u.byConversation) ? u.byConversation : []).forEach(function (c) {
            if (!c || typeof c.addr !== 'string' || !c.addr) return
            var e = next[c.addr] || (next[c.addr] = { unread: 0, waiting: 0 })
            e.unread += c.muted ? 0 : (c.unread || 0)
            e.waiting += c.waiting || 0
          })
          unread = next
          unreadOk = true
        }, function () {
          // The counts keep their last value.
        }).then(function () {
          if (stopped) return
          draw()
          if (ctx.visible()) timer = setTimeout(readUnread, UNREAD_MS)
        })
      }

      function onRows(list) {
        rows = (Array.isArray(list) ? list : []).filter(function (r) { return r && typeof r.address === 'string' && r.address })
        var key = rows.map(function (r) { return r.address + '@' + (r.preview && r.preview.seq != null ? r.preview.seq : '') }).join(',')
        draw()
        if (key !== seqs) {
          seqs = key
          readUnread()
        }
      }

      search.addEventListener('input', function () {
        filter = search.value
        draw()
      })
      ctx.onVisible(function (v) { if (v) readUnread(); else clearTimeout(timer) })

      if (ctx.can('layout.get')) {
        k2.layout.get().then(function (info) { column = threadColumn(info) }, function () { column = null })
        if (ctx.can('layout.subscribe')) offLayout = k2.layout.subscribe(function (info) { column = threadColumn(info) })
      }
      if (!ctx.can('thread.unread')) ctx.note('Unread counts aren’t available here.')
      off = k2.agents.subscribe(onRows, function (err) {
        rows = []
        draw()
        ctx.note('K2 didn’t list the agents (' + words(err) + ').')
      })
      draw()

      return {
        stop: function () {
          stopped = true
          clearTimeout(timer)
          if (typeof off === 'function') off()
          if (typeof offLayout === 'function') offLayout()
        },
      }
    },
  })
})()
