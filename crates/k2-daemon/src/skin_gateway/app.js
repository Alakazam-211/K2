const roomsEl = document.getElementById("rooms");
const logEl = document.getElementById("log");
const whoEl = document.getElementById("who");
const textEl = document.getElementById("text");
let current = { handle: "", conversation: "" };
let ws = null;

// Image attachments show as images (thread-images-v1). Only ever through
// an <img> (an SVG comes back sanitized and never runs script there).
const IMAGE_MIMES = new Set([
  "image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp",
  "image/tiff", "image/heic", "image/heif", "image/svg+xml",
]);

function fmtSize(n) {
  if (typeof n !== "number") return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function attachmentUrl(docId, n, mode, extra) {
  const q = new URLSearchParams({ addr: current.handle, id: docId, n: String(n), mode, ...(extra || {}) });
  return `/cli/thread/attachment?${q}`;
}

async function download(docId, n, name) {
  const parts = [];
  let offset = 0;
  for (;;) {
    const r = await api(attachmentUrl(docId, n, "range", { offset: String(offset), len: String(4 * 1024 * 1024) }));
    if (!r.ok) return;
    const j = await r.json();
    const bin = atob(j.base64 || "");
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    parts.push(bytes);
    offset += j.len || 0;
    if (j.eof || !j.len) break;
  }
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob(parts));
  a.download = name || "file";
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 10000);
}

function attachmentEl(docId, n, att) {
  const el = document.createElement("div");
  el.className = "att";
  if (!att || att.state === "hidden") {
    el.classList.add("hidden");
    el.textContent = "A file (not shared with you)";
    return el;
  }
  if (att.state === "gone") {
    el.classList.add("gone");
    el.textContent = `${att.name || "file"} — not found (moved or deleted)`;
    return el;
  }
  const label = document.createElement("span");
  label.textContent = `${att.name || "file"} ${fmtSize(att.size)}${att.state === "changed" ? " · changed since sent" : ""}`;
  if (IMAGE_MIMES.has(att.mime)) {
    const img = document.createElement("img");
    img.alt = att.name || "image";
    img.loading = "lazy";
    api(attachmentUrl(docId, n, "thumb", { px: "512" }))
      .then((r) => (r.ok ? r.json() : null))
      .then((j) => {
        if (j && j.base64 && j.mime) {
          img.src = `data:${j.mime};base64,${j.base64}`;
          el.prepend(img);
        }
      })
      .catch(() => {});
    img.addEventListener("click", async () => {
      const r = await api(attachmentUrl(docId, n, "preview"));
      if (!r.ok) return;
      const j = await r.json();
      if (!j.base64 || !j.mime) return;
      const w = window.open("", "_blank", "noopener");
      if (!w) return;
      const big = w.document.createElement("img");
      big.src = `data:${j.mime};base64,${j.base64}`;
      big.style.maxWidth = "100%";
      w.document.body.appendChild(big);
    });
  } else {
    el.classList.add("chip");
    el.addEventListener("click", () => download(docId, n, att.name));
  }
  el.appendChild(label);
  return el;
}

function message(doc) {
  if (!doc) return;
  const t = doc.text || doc.body;
  const atts = Array.isArray(doc.attachments) ? doc.attachments : [];
  if (!t && atts.length === 0) return;
  const li = document.createElement("li");
  if (t) {
    const p = document.createElement("div");
    p.textContent = String(t);
    li.appendChild(p);
  }
  atts.forEach((a, n) => li.appendChild(attachmentEl(doc.id, n, a)));
  logEl.appendChild(li);
  logEl.scrollTop = logEl.scrollHeight;
}

async function api(path, opts) {
  const r = await fetch(path, { credentials: "same-origin", ...opts });
  if (r.status === 401) {
    location.href = "/login";
    throw new Error("login");
  }
  return r;
}

document.getElementById("out").addEventListener("click", async () => {
  await fetch("/logout", { method: "POST", credentials: "same-origin" });
  location.href = "/login";
});

document.getElementById("post").addEventListener("submit", async (e) => {
  e.preventDefault();
  const text = textEl.value.trim();
  if (!text || !current.handle) return;
  const r = await api("/cli/thread/post", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ addr: current.handle, text }),
  });
  if (r.ok) textEl.value = "";
});

function openWs(conversation) {
  if (ws) { ws.close(); ws = null; }
  if (!conversation) return;
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/cli/overlay/events?conversation=${encodeURIComponent(conversation)}`);
  ws.onmessage = (ev) => {
    try {
      const f = JSON.parse(ev.data);
      message(f && f.doc);
    } catch (_) {}
  };
}

async function openRoom(handle) {
  current.handle = handle;
  for (const b of roomsEl.querySelectorAll("button")) {
    b.classList.toggle("on", b.dataset.handle === handle);
  }
  logEl.innerHTML = "";
  const r = await api(`/cli/thread?addr=${encodeURIComponent(handle)}`);
  const j = await r.json();
  current.conversation = j.conversation_id || "";
  for (const it of j.items || []) message(it.doc);
  const last = (j.items || []).slice(-1)[0];
  if (last) {
    // This guest's own read cursor (app-thread-parity-v1); older servers 403.
    api("/cli/thread/read", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ addr: handle, seq: last.seq }),
    }).catch(() => {});
  }
  openWs(current.conversation);
}

async function boot() {
  const r = await api("/cli/skin/agents");
  const j = await r.json();
  const agents = j.agents || [];
  whoEl.textContent = agents.length ? "" : "no rooms on this pass";
  roomsEl.innerHTML = "";
  for (const a of agents) {
    const b = document.createElement("button");
    const handle = a.handle || a.roomHandle || "";
    b.dataset.handle = handle;
    b.textContent = a.displayName || handle;
    b.addEventListener("click", () => openRoom(handle));
    roomsEl.appendChild(b);
  }
  if (agents[0]) openRoom(agents[0].handle || agents[0].roomHandle || "");
}

boot().catch(() => {});
