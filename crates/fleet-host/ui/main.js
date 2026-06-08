// Fleet host face frontend. No bundler: `withGlobalTauri` exposes window.__TAURI__,
// so we use the event + core APIs directly from this static file.
const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

const inboxEl = document.getElementById("inbox");
const statusEl = document.getElementById("status");

const LOC_GLYPH = { laptop: "💻", docker: "🐳", remote: "☁️" };

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

function renderTab(t) {
  const row = el("div", `tab ${t.state}${t.attention ? " attention" : ""}${t.muted ? " muted" : ""}`);

  row.appendChild(el("span", "glyph", t.state_glyph));

  const body = el("div", "body");
  const title = el("div", "title", t.title || t.session_id);
  body.appendChild(title);

  const sub = el("div", "sub");
  const loc = LOC_GLYPH[t.location] || "•";
  sub.appendChild(el("span", null, `${loc} ${t.agent || "agent"}`));
  sub.appendChild(el("span", null, t.state));
  if (t.run_count > 1) sub.appendChild(el("span", null, `${t.run_count} runs`));
  if (t.confidence) sub.appendChild(el("span", null, `conf: ${t.confidence}`));
  body.appendChild(sub);
  row.appendChild(body);

  if (t.attention) {
    row.appendChild(el("span", "badge attention", "waiting"));
  }
  const dot = el("span", `dot${t.unread ? "" : " hidden"}`);
  row.appendChild(dot);

  return row;
}

function render(inbox) {
  inbox = inbox || { tabs: [], waiting_count: 0, connected: false };

  statusEl.textContent = inbox.connected
    ? (inbox.waiting_count > 0 ? `${inbox.waiting_count} waiting` : "connected")
    : "disconnected";
  statusEl.className = "status " + (inbox.connected ? "connected" : "disconnected");

  inboxEl.replaceChildren();
  if (!inbox.tabs || inbox.tabs.length === 0) {
    inboxEl.appendChild(el("p", "empty", inbox.connected ? "No sessions yet." : "Waiting for the Hub…"));
    return;
  }
  for (const t of inbox.tabs) inboxEl.appendChild(renderTab(t));
}

// Live updates.
listen("inbox", (e) => render(e.payload));

// Initial pull of whatever the Hub link already has.
invoke("get_inbox").then(render).catch(() => render(null));
