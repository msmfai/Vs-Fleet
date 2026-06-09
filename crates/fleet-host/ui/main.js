// Fleet rail — the Discord-style list of VS Code server workspaces. Servers
// appear by phoning home (push); clicking one swaps the embedded editor surface.
// Spawning shows an optimistic "pending" tab immediately, which resolves when the
// new server phones in. Agent state comes from the Hub `inbox` event (id ==
// session_id).
const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

const railEl = document.getElementById("rail");
const statusEl = document.getElementById("status");
const spawnBtn = document.getElementById("spawn");
if (spawnBtn) spawnBtn.onclick = spawnServer;

const STATE_GLYPH = { working: "▶", waiting: "⏸", idle: "·", done: "✓", error: "✕", dead: "☠" };

let servers = [];          // registered (phoned home) — from the backend
let pending = [];          // [{id, label}] spawned but not yet registered
let selected = null;
let inbox = { tabs: [], waiting_count: 0, connected: false };
let statusOverride = null;

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

function agentFor(id) {
  return (inbox.tabs || []).find((t) => t.session_id === id);
}

function waitingAge(iso) {
  if (!iso) return "waiting";
  const ms = Date.now() - Date.parse(iso);
  if (!(ms >= 0)) return "waiting";
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  return m < 60 ? `${m}m` : `${Math.floor(m / 60)}h`;
}

// Registered servers + still-pending ones (those that haven't phoned home yet).
function displayed() {
  const regIds = new Set(servers.map((s) => s.id));
  const stillPending = pending.filter((p) => !regIds.has(p.id)).map((p) => ({ ...p, pending: true }));
  return [...servers.map((s) => ({ ...s, pending: false })), ...stillPending];
}

function render() {
  statusEl.textContent = statusOverride || (
    inbox.connected
      ? inbox.waiting_count > 0 ? `${inbox.waiting_count} waiting` : "connected"
      : "disconnected"
  );
  statusEl.className = "status " + (
    statusOverride ? "disconnected" : inbox.connected ? (inbox.waiting_count ? "waiting" : "connected") : "disconnected"
  );

  const list = displayed();
  railEl.replaceChildren();
  if (!list.length) {
    railEl.appendChild(el("p", "empty", "No servers yet — press + to start one."));
    return;
  }

  for (const srv of list) {
    const a = srv.pending ? null : agentFor(srv.id);
    const state = a ? a.state : "idle";
    const attention = a ? a.attention : false;

    const row = el(
      "div",
      `srv ${state}${attention ? " attention" : ""}${srv.id === selected ? " selected" : ""}${srv.pending ? " pending" : ""}`
    );
    row.onclick = () => selectServer(srv.id);

    if (srv.pending) row.appendChild(el("span", "glyph spinner", ""));
    else row.appendChild(el("span", "glyph", STATE_GLYPH[state] || "·"));

    const body = el("div", "body");
    body.appendChild(el("div", "label", srv.label));
    const sub = el("div", "sub");
    if (srv.pending) sub.appendChild(el("span", "preview muted", "starting…"));
    else if (a && a.last_message) sub.appendChild(el("span", "preview", a.last_message));
    else sub.appendChild(el("span", "preview muted", a ? state : "vscode server"));
    body.appendChild(sub);
    row.appendChild(body);

    const right = el("div", "right");
    if (attention) right.appendChild(el("span", "badge", waitingAge(a && a.waiting_since)));
    else if (a && a.unread) right.appendChild(el("span", "dot"));
    const close = el("button", "close", "×");
    close.title = "Close server";
    close.onclick = (ev) => { ev.stopPropagation(); closeServer(srv.id); };
    right.appendChild(close);
    row.appendChild(right);

    railEl.appendChild(row);
  }
}

async function spawnServer() {
  statusOverride = null;
  render();
  try {
    const id = await invoke("spawn_server");
    pending.push({ id, label: id });
    selectServer(id); // shows the loading page + selects the pending tab
  } catch (e) {
    statusOverride = `spawn failed: ${String(e)}`;
    render();
    setTimeout(() => { statusOverride = null; render(); }, 8000);
  }
}

function closeServer(id) {
  pending = pending.filter((p) => p.id !== id);
  invoke("close_server", { id }).catch(() => {});
  render();
}

async function selectServer(id) {
  selected = id;
  render();
  try { await invoke("select_server", { id }); } catch (e) { /* ignore */ }
}

async function refreshServers() {
  try {
    servers = await invoke("get_servers");
    selected = await invoke("selected_server");
  } catch (e) {
    servers = [];
  }
  // Drop pending entries that have now phoned home.
  pending = pending.filter((p) => !servers.some((s) => s.id === p.id));

  const ids = displayed().map((s) => s.id);
  // Auto-select the first server once any exist and nothing valid is selected.
  if ((!selected || !ids.includes(selected)) && servers.length) {
    selectServer(servers[0].id);
    return;
  }
  // If the selected server just became registered (was loading), navigate to it.
  if (selected && servers.some((s) => s.id === selected)) {
    invoke("select_server", { id: selected }).catch(() => {});
  }
  render();
}

// Called from Rust (mux::select) so the rail highlight stays in sync.
window.__fleetSyncSelection = async () => {
  try { selected = await invoke("selected_server"); render(); } catch (e) { /* ignore */ }
};

listen("servers-changed", () => refreshServers());
listen("inbox", (e) => { inbox = e.payload || inbox; render(); });
refreshServers();
setInterval(render, 1000); // tick the "waiting Nm" age
