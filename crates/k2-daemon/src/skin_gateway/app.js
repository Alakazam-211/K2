const roomsEl = document.getElementById("rooms");
const logEl = document.getElementById("log");
const whoEl = document.getElementById("who");
const textEl = document.getElementById("text");
let current = { handle: "", conversation: "" };
let ws = null;

function line(text) {
  const li = document.createElement("li");
  li.textContent = text;
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
      const t = f && f.doc && (f.doc.text || f.doc.body);
      if (t) line(String(t));
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
  for (const it of j.items || []) {
    const t = it.doc && (it.doc.text || it.doc.body);
    if (t) line(String(t));
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
