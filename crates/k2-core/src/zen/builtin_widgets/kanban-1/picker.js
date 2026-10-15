// Agent picker for the Kanban's Thread panel. A sealed widget can't import
// K2's React components, so this mirrors the look and keys of Garden 1's
// "Add agent" picker: components/Zen/ZenAddAgent.tsx → Home/HomeAddPanels.tsx
// AddAgentPicker → components/ui/SearchableAgentList.tsx (search box on top,
// section headers with an accent bar + count, rows of a 20px square avatar
// (picture, else the first letter on a colour, as Sidebar/ProjectAvatar),
// name, server tag, and here a working dot; ↑/↓ walk the rows and clamp,
// Enter picks, Esc closes; selected row accent 15%, keyboard row white 6%).
(function () {
  const COLORS = ["#3b82f6", "#a855f7", "#14b8a6", "#f59e0b", "#ef4444", "#22c55e", "#ec4899", "#6366f1"];

  function hash(s) {
    let h = 2166136261;
    for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619); }
    return h >>> 0;
  }

  function el(tag, cls, text) {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  }

  // ProjectAvatar's look: a square picture, else the first letter on the
  // agent's colour (agent rows carry `image` and `color`).
  function avatar(row, size) {
    const box = el("span", "ap-avatar");
    box.style.width = size + "px";
    box.style.height = size + "px";
    const pic = typeof row.image === "string" && row.image.indexOf("data:image/") === 0 ? row.image
      : typeof row.avatar === "string" && row.avatar.indexOf("data:image/") === 0 ? row.avatar : "";
    if (pic) {
      const img = document.createElement("img");
      img.alt = "";
      img.src = pic;
      box.appendChild(img);
    } else {
      box.textContent = String(row.label || row.address || "?").charAt(0).toUpperCase();
      box.style.backgroundColor = typeof row.color === "string" && /^#[0-9a-fA-F]{3,8}$/.test(row.color)
        ? row.color : COLORS[hash(row.address || "") % COLORS.length];
      box.style.fontSize = Math.round(size * 0.5) + "px";
    }
    return box;
  }

  function stateOf(row) {
    if (row.state === "unreachable" || row.activity === "unreachable") return "unreachable";
    if (row.needsYou || row.activity === "needs-you") return "needs-you";
    if (row.working || row.activity === "working" || row.activity === "monitoring") return "working";
    return "idle";
  }

  function dot(row) {
    const s = stateOf(row);
    const d = el("span", "ap-dot ap-dot-" + s);
    d.title = s === "needs-you" ? "Needs you" : s === "working" ? "Working" : s === "unreachable" ? "Not reachable" : "Idle";
    return d;
  }

  // create({root, onPick}) → { setRows(rows, selectedAddress), close() }
  function create(opts) {
    const root = opts.root;
    const onPick = opts.onPick;
    root.replaceChildren();

    const trigger = el("button", "ap-trigger");
    trigger.type = "button";
    trigger.setAttribute("aria-haspopup", "listbox");
    trigger.setAttribute("aria-expanded", "false");
    trigger.title = "Pick the agent to talk to";
    root.appendChild(trigger);

    const panel = el("div", "ap-panel");
    panel.hidden = true;
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-label", "Pick an agent");
    const searchWrap = el("div", "ap-search-wrap");
    const search = document.createElement("input");
    search.type = "text";
    search.className = "ap-search";
    search.placeholder = "Search agents…";
    search.autocomplete = "off";
    search.spellcheck = false;
    searchWrap.appendChild(search);
    const list = el("div", "ap-list");
    list.setAttribute("role", "listbox");
    list.setAttribute("aria-label", "Agents");
    panel.appendChild(searchWrap);
    panel.appendChild(list);
    root.appendChild(panel);

    let rows = [];
    let selected = "";
    let kbd = -1;
    let flat = [];

    function paintTrigger() {
      trigger.replaceChildren();
      const row = rows.find(function (r) { return r.address === selected; });
      if (row) {
        trigger.appendChild(avatar(row, 20));
        trigger.appendChild(el("span", "ap-name", row.label || row.address));
        if (row.server) trigger.appendChild(el("span", "ap-server", row.server));
        trigger.appendChild(dot(row));
      } else {
        trigger.appendChild(el("span", "ap-name", selected ? selected : "Pick an agent"));
      }
      trigger.appendChild(el("span", "ap-chev", "▾"));
    }

    function matches(row, q) {
      if (!q) return true;
      return ((row.label || "") + " " + row.address + " " + (row.server || "")).toLowerCase().indexOf(q) !== -1;
    }

    function paintList() {
      const q = (search.value || "").trim().toLowerCase();
      list.replaceChildren();
      flat = [];
      const groups = [];
      const byKey = {};
      rows.forEach(function (r) {
        if (!matches(r, q)) return;
        const key = r.server || "";
        if (!byKey[key]) { byKey[key] = { name: key ? key : "This computer", rows: [] }; groups.push(byKey[key]); }
        byKey[key].rows.push(r);
      });
      if (!flat.length && !groups.length) {
        list.appendChild(el("div", "ap-empty", q ? "No agents match" : "No agents in reach"));
        return;
      }
      groups.forEach(function (g) {
        const head = el("div", "ap-head");
        head.appendChild(el("span", "ap-bar"));
        head.appendChild(el("span", "ap-head-name", g.name));
        head.appendChild(el("span", "ap-count", String(g.rows.length)));
        list.appendChild(head);
        g.rows.forEach(function (r) {
          const i = flat.length;
          flat.push(r.address);
          const b = el("button", "ap-row");
          b.type = "button";
          b.tabIndex = -1;
          b.setAttribute("role", "option");
          b.dataset.address = r.address;
          b.setAttribute("aria-selected", r.address === selected ? "true" : "false");
          if (r.address === selected) b.classList.add("selected");
          if (i === kbd) b.classList.add("kbd");
          b.appendChild(avatar(r, 20));
          b.appendChild(el("span", "ap-name", r.label || r.address));
          if (r.server) b.appendChild(el("span", "ap-server", r.server));
          b.appendChild(dot(r));
          b.title = r.address + (r.stateLabel ? " — " + r.stateLabel : "");
          b.addEventListener("mousedown", function (e) { e.preventDefault(); });
          b.addEventListener("click", function () { pick(r.address); });
          list.appendChild(b);
        });
      });
      const k = list.querySelector(".ap-row.kbd");
      if (k && typeof k.scrollIntoView === "function") k.scrollIntoView({ block: "nearest" });
    }

    function isOpen() { return !panel.hidden; }

    function open() {
      if (isOpen()) return;
      panel.hidden = false;
      trigger.setAttribute("aria-expanded", "true");
      search.value = "";
      kbd = -1;
      paintList();
      kbd = Math.max(0, flat.indexOf(selected));
      paintList();
      search.focus();
    }

    function close(refocus) {
      if (!isOpen()) return;
      panel.hidden = true;
      trigger.setAttribute("aria-expanded", "false");
      if (refocus) trigger.focus();
    }

    function pick(address) {
      close(true);
      if (address && address !== selected) onPick(address);
    }

    trigger.addEventListener("click", function () { isOpen() ? close(true) : open(); });
    trigger.addEventListener("keydown", function (e) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); open(); }
    });
    search.addEventListener("input", function () { kbd = 0; paintList(); });
    search.addEventListener("keydown", function (e) {
      if (e.key === "ArrowDown") { e.preventDefault(); kbd = Math.min(kbd + 1, flat.length - 1); paintList(); }
      else if (e.key === "ArrowUp") { e.preventDefault(); kbd = Math.max(kbd - 1, 0); paintList(); }
      else if (e.key === "Enter") { e.preventDefault(); if (kbd >= 0 && kbd < flat.length) pick(flat[kbd]); }
      else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(true); }
    });
    document.addEventListener("mousedown", function (e) {
      if (isOpen() && !root.contains(e.target)) close(false);
    }, true);

    paintTrigger();
    return {
      setRows: function (next, sel) {
        rows = Array.isArray(next) ? next.slice() : [];
        selected = sel || "";
        paintTrigger();
        if (isOpen()) paintList();
      },
      close: function () { close(false); },
      isOpen: isOpen
    };
  }

  window.KanbanPicker = { create: create };
})();
