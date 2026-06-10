// Fleet rail — the Discord-style list of VS Code server workspaces. Servers
// appear by phoning home (push); clicking one swaps the embedded editor surface.
// Spawning shows an optimistic "pending" tab immediately, which resolves when the
// new server phones in. Agent state comes from the Hub `inbox` event (id ==
// session_id).
const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

const railEl = document.getElementById("rail");
const statusEl = document.getElementById("status");
const statusDetailEl = document.getElementById("status-detail");
const spawnBtn = document.getElementById("spawn");
const jumpBtn = document.getElementById("jump");
const paletteBtn = document.getElementById("palette-open");
const paletteEl = document.getElementById("palette");
const paletteInput = document.getElementById("palette-input");
const paletteList = document.getElementById("palette-list");
if (spawnBtn) spawnBtn.onclick = spawnServer;
if (jumpBtn) jumpBtn.onclick = jumpNextUnread;
if (paletteBtn) paletteBtn.onclick = () => openPalette();

const STATE_GLYPH = { working: "▶", waiting: "⏸", idle: "·", done: "✓", error: "✕", dead: "☠" };
const URGENCY_LABEL = { approval: "approval", question: "question", "idle-done": "done" };
const PENDING_SLOW_MS = 15_000;
const PENDING_TIMEOUT_MS = 45_000;
const STATUS_CLEAR_MS = 8_000;
const STATUS_CLEAR_BY_LEVEL_MS = { error: 30_000, warning: 20_000, info: 10_000 };

let servers = [];          // registered (phoned home) — from the backend
let pending = [];          // [{id, label, startedAt}] spawned but not yet registered
let selected = null;
let inbox = { tabs: [], waiting_count: 0, connected: false };
let statusOverride = null;
let statusTimer = null;
let spawning = false;
const closing = new Set();
const sessionActions = new Set();
let paletteOpen = false;
let paletteQuery = "";
let paletteIndex = 0;

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

function token(value) {
  return value == null ? "" : String(value).toLowerCase();
}

function agentFor(id) {
  return (inbox.tabs || []).find((t) => t.session_id === id);
}

function waitingAge(iso) {
  if (!iso) return "waiting";
  const ms = Date.now() - Date.parse(iso);
  if (!(ms >= 0)) return "waiting";
  return formatAge(ms);
}

function formatAge(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  return m < 60 ? `${m}m` : `${Math.floor(m / 60)}h`;
}

function pendingAgeMs(srv) {
  return Date.now() - (srv.startedAt || Date.now());
}

function pendingVisual(srv) {
  const age = pendingAgeMs(srv);
  if (age >= PENDING_TIMEOUT_MS) {
    return { state: "error", level: "timed-out", text: `start timed out (${formatAge(age)})` };
  }
  if (age >= PENDING_SLOW_MS) {
    return { state: "waiting", level: "slow", text: `still starting ${formatAge(age)}` };
  }
  return { state: "working", level: "", text: `starting ${formatAge(age)}` };
}

function displayLabel(srv) {
  return srv.label || srv.id;
}

function isOwned(srv) {
  return srv && srv.owned !== false;
}

function rowTitle(srv, agent) {
  const title = agent && agent.title && agent.title.trim();
  return title || displayLabel(srv);
}

function agentMeta(agent) {
  if (!agent) return "";
  const parts = [];
  if (agent.agent) parts.push(agent.agent);
  if (agent.location) parts.push(agent.location);
  if (agent.run_count > 1) parts.push(`${agent.run_count} runs`);
  return parts.join(" · ");
}

function serverState(agent) {
  if (agent) return agent.state || "idle";
  return inbox.connected ? "idle" : "dead";
}

function serverPreview(agent, state) {
  const meta = agentMeta(agent);
  if (!agent) return inbox.connected ? "no agent activity" : "hub disconnected";
  if (agent.last_message) return inbox.connected ? agent.last_message : `last: ${agent.last_message}`;
  if (!inbox.connected) return meta ? `${meta} · last known ${state}` : `last known ${state}`;
  return meta ? `${meta} · ${state}` : state;
}

function canDismissAgent(agent, state) {
  return Boolean(agent && (state === "dead" || state === "error"));
}

function serverRowModel(srv) {
  const agent = srv.pending ? null : agentFor(srv.id);
  const pendingState = srv.pending ? pendingVisual(srv) : null;
  const isClosing = closing.has(srv.id);
  const title = rowTitle(srv, agent);
  const state = isClosing ? "waiting" : pendingState ? pendingState.state : serverState(agent);
  const preview = isClosing
    ? "closing…"
    : pendingState ? pendingState.text : serverPreview(agent, state);
  return {
    srv,
    agent,
    pendingState,
    isClosing,
    title,
    state,
    preview,
    attention: agent ? agent.attention : false,
    flags: stateFlags(agent),
  };
}

function searchableFields(model) {
  const agent = model.agent;
  return [
    model.srv.id,
    model.srv.label,
    model.title,
    model.state,
    model.preview,
    agent && agent.agent,
    agent && agent.location,
    agent && agent.urgency,
    agent && agent.confidence,
    agent && agent.last_message,
  ].filter(Boolean).map((v) => String(v));
}

function fuzzyTokenScore(tokenText, targetText) {
  const query = token(tokenText);
  const target = token(targetText);
  if (!query) return 0;
  if (!target) return null;

  let qi = 0;
  let score = 0;
  let prev = -2;
  for (let ti = 0; ti < target.length && qi < query.length; ti += 1) {
    if (target[ti] !== query[qi]) continue;
    if (ti === 0) score += 10;
    if (ti === prev + 1) score += 5;
    prev = ti;
    qi += 1;
  }
  return qi === query.length ? score : null;
}

function paletteScore(queryText, model) {
  const tokens = token(queryText).split(/\s+/).filter(Boolean);
  if (!tokens.length) return 0;

  let score = 0;
  const fields = searchableFields(model);
  for (const part of tokens) {
    let best = null;
    for (const field of fields) {
      const next = fuzzyTokenScore(part, field);
      if (next != null) best = best == null ? next : Math.max(best, next);
    }
    if (best == null) return null;
    score += best;
  }
  if (model.agent && model.agent.unread) score += 30;
  if (model.attention) score += 20;
  if (model.srv.id === selected) score += 3;
  return score;
}

function paletteCandidates() {
  return displayed()
    .map((srv, index) => ({ ...serverRowModel(srv), order: index }))
    .map((model) => ({ ...model, score: paletteScore(paletteQuery, model) }))
    .filter((model) => model.score != null)
    .sort((a, b) => b.score - a.score || a.order - b.order);
}

function attentionCandidates() {
  const rows = displayed().map(serverRowModel);
  const unread = rows.filter((row) => row.agent && row.agent.unread);
  return unread.length ? unread : rows.filter((row) => row.attention);
}

function nextAttentionCandidate() {
  const candidates = attentionCandidates();
  if (!candidates.length) return null;
  const current = candidates.findIndex((row) => row.srv.id === selected);
  return candidates[(current + 1 + candidates.length) % candidates.length];
}

function attentionUrgency(agent) {
  const urgency = token(agent && agent.urgency);
  if (!urgency || urgency === "null") return null;
  return {
    token: urgency,
    label: URGENCY_LABEL[urgency] || urgency,
  };
}

function confidenceClass(agent) {
  const confidence = token(agent && agent.confidence);
  if (confidence === "inferred") return "confidence-inferred";
  if (confidence === "high") return "confidence-high";
  return "";
}

function confidenceTitle(agent) {
  const confidence = token(agent && agent.confidence);
  if (confidence === "inferred") return "inferred";
  if (confidence === "high") return "high confidence";
  return "";
}

function stateFlags(agent) {
  if (!agent) return [];
  if (agent.soloed) return ["solo"];
  if (agent.muted) return ["muted"];
  return [];
}

function appendStateBadges(parent, agent) {
  for (const flag of stateFlags(agent)) {
    const chip = el("span", `state-chip ${flag}`, flag);
    chip.title = flag;
    parent.appendChild(chip);
  }
}

function appendAttentionBadges(parent, agent) {
  const urgency = attentionUrgency(agent);
  if (urgency) {
    const chip = el("span", `urgency-chip urgency-${urgency.token}`, urgency.label);
    chip.title = urgency.label;
    parent.appendChild(chip);
  }

  const confidence = confidenceClass(agent);
  const badge = el("span", `badge${confidence ? ` ${confidence}` : ""}`, waitingAge(agent && agent.waiting_since));
  const title = confidenceTitle(agent);
  if (title) badge.title = title;
  parent.appendChild(badge);
}

function actionKey(id, action) {
  return `${action}:${id}`;
}

function sessionActionButton(action, text, title, active, disabled, toggle = true) {
  const btn = el("button", `session-action ${action}${active ? " active" : ""}`, text);
  btn.type = "button";
  btn.title = title;
  btn.dataset.action = action;
  btn.setAttribute("aria-label", title);
  if (toggle) btn.setAttribute("aria-pressed", active ? "true" : "false");
  btn.disabled = disabled;
  return btn;
}

// Registered servers + still-pending ones (those that haven't phoned home yet).
function displayed() {
  const regIds = new Set(servers.map((s) => s.id));
  const stillPending = pending.filter((p) => !regIds.has(p.id)).map((p) => ({ ...p, pending: true }));
  const tabOrder = new Map((inbox.tabs || []).map((tab, index) => [tab.session_id, index]));
  return [...servers.map((s) => ({ ...s, pending: false })), ...stillPending]
    .map((srv, index) => ({
      srv,
      index,
      rank: tabOrder.has(srv.id) ? tabOrder.get(srv.id) : 10_000 + index,
    }))
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .map((entry) => entry.srv);
}

function railStatus(list) {
  if (statusOverride && statusOverride.message) {
    return {
      message: statusOverride.message,
      level: statusOverride.level || "error",
      title: statusOverride.source ? `${statusOverride.source}: ${statusOverride.message}` : statusOverride.message,
    };
  }
  const timedOut = list.filter((srv) => srv.pending && pendingAgeMs(srv) >= PENDING_TIMEOUT_MS);
  if (timedOut.length) {
    return {
      message: timedOut.length === 1 ? "start timeout" : `${timedOut.length} timeouts`,
      level: "warning",
      title: timedOut.map(displayLabel).join(", "),
    };
  }
  if (spawning) return { message: "starting", level: "waiting", title: "Starting server" };
  if (closing.size) return { message: "closing", level: "waiting", title: "Closing server" };
  if (!inbox.connected) return { message: "disconnected", level: "disconnected", title: "Hub disconnected" };
  if (inbox.waiting_count > 0) return { message: `${inbox.waiting_count} waiting`, level: "waiting", title: "" };
  return { message: "connected", level: "connected", title: "" };
}

function updateSpawnButton() {
  if (!spawnBtn) return;
  spawnBtn.disabled = spawning;
  spawnBtn.classList.toggle("busy", spawning);
  spawnBtn.title = spawning ? "Starting server" : "New Server";
  spawnBtn.setAttribute("aria-label", spawnBtn.title);
  spawnBtn.setAttribute("aria-busy", spawning ? "true" : "false");
}

function updateToolbarButtons() {
  if (jumpBtn) jumpBtn.disabled = !attentionCandidates().length;
  if (paletteBtn) paletteBtn.disabled = !displayed().length;
}

function statusClearDelay(level) {
  return STATUS_CLEAR_BY_LEVEL_MS[level] || STATUS_CLEAR_MS;
}

function clearStatusOverride() {
  if (!statusOverride) return;
  if (statusTimer) clearTimeout(statusTimer);
  statusTimer = null;
  const message = statusOverride.message;
  statusOverride = null;
  render();
  invoke("clear_host_status_if_current", { message }).catch(() => {});
}

function renderStatusDetail() {
  if (!statusDetailEl) return;
  statusDetailEl.replaceChildren();
  if (!statusOverride || !statusOverride.message) {
    statusDetailEl.className = "status-detail hidden";
    return;
  }

  statusDetailEl.className = `status-detail ${statusOverride.level || "error"}`;
  const msg = el("div", "status-detail-message", statusOverride.message);
  const close = el("button", "status-detail-close", "×");
  close.type = "button";
  close.title = "Dismiss";
  close.setAttribute("aria-label", "Dismiss status");
  close.onclick = clearStatusOverride;
  statusDetailEl.appendChild(msg);
  statusDetailEl.appendChild(close);
}

function render() {
  const list = displayed();
  const status = railStatus(list);
  statusEl.textContent = status.message;
  statusEl.title = status.title || "";
  statusEl.className = `status ${status.level}`;
  updateSpawnButton();
  updateToolbarButtons();
  renderStatusDetail();

  const activeEl = document.activeElement;
  const activeServerId = activeEl && activeEl.closest
    ? activeEl.closest(".srv")?.dataset.serverId
    : null;
  const activeWasClose = activeEl && activeEl.classList && activeEl.classList.contains("close");
  const activeAction = activeEl && activeEl.dataset ? activeEl.dataset.action : null;
  let focusAfterRender = null;

  railEl.replaceChildren();
  railEl.setAttribute("role", "list");
  if (!list.length) {
    railEl.appendChild(el("p", "empty", inbox.connected ? "No servers." : "Connecting."));
    return;
  }

  for (const model of list.map(serverRowModel)) {
    const { srv, agent: a, pendingState, isClosing, title, state, attention, flags, preview } = model;

    const row = el(
      "div",
      `srv ${state}${attention ? " attention" : ""}${flags.includes("muted") ? " muted-state" : ""}${flags.includes("solo") ? " soloed-state" : ""}${srv.id === selected ? " selected" : ""}${srv.pending ? " pending" : ""}${pendingState && pendingState.level ? ` ${pendingState.level}` : ""}${isClosing ? " closing" : ""}`
    );
    row.dataset.serverId = srv.id;
    row.tabIndex = 0;
    row.setAttribute("role", "listitem");
    row.setAttribute("aria-current", srv.id === selected ? "true" : "false");
    row.setAttribute("aria-label", `${title}, ${preview}${flags.length ? `, ${flags.join(", ")}` : ""}`);
    row.onclick = () => selectServer(srv.id);
    row.onkeydown = (ev) => handleRowKeydown(ev, row, srv.id);

    if (srv.pending && pendingState.state !== "error") row.appendChild(el("span", "glyph spinner", ""));
    else row.appendChild(el("span", "glyph", STATE_GLYPH[state] || "·"));

    const body = el("div", "body");
    body.appendChild(el("div", "label", title));
    const sub = el("div", "sub");
    if (isClosing) sub.appendChild(el("span", "preview muted", "closing…"));
    else if (pendingState) sub.appendChild(el("span", `preview ${pendingState.state === "error" ? "error-text" : "muted"}`, pendingState.text));
    else sub.appendChild(el("span", `preview ${a && a.last_message && inbox.connected ? "" : "muted"}`, preview));
    body.appendChild(sub);
    row.appendChild(body);

    const right = el("div", "right");
    appendStateBadges(right, a);
    if (attention) appendAttentionBadges(right, a);
    else if (a && a.unread) right.appendChild(el("span", "dot"));
    if (a) {
      const mutePending = sessionActions.has(actionKey(srv.id, "mute"));
      const soloPending = sessionActions.has(actionKey(srv.id, "solo"));
      const actionDisabled = !inbox.connected;
      const mute = sessionActionButton(
        "mute",
        a.muted ? "U" : "M",
        a.muted ? `Unmute ${title}` : `Mute ${title}`,
        Boolean(a.muted),
        actionDisabled || mutePending
      );
      mute.onclick = (ev) => {
        ev.stopPropagation();
        setSessionMuted(srv.id, !a.muted);
      };
      const solo = sessionActionButton(
        "solo",
        a.soloed ? "◉" : "◎",
        a.soloed ? `Clear solo ${title}` : `Solo ${title}`,
        Boolean(a.soloed),
        actionDisabled || soloPending
      );
      solo.onclick = (ev) => {
        ev.stopPropagation();
        setSessionSoloed(srv.id, !a.soloed);
      };
      right.appendChild(mute);
      right.appendChild(solo);
      if (srv.id === activeServerId && activeAction === "mute") focusAfterRender = mute;
      if (srv.id === activeServerId && activeAction === "solo") focusAfterRender = solo;
      if (canDismissAgent(a, state)) {
        const dismissPending = sessionActions.has(actionKey(srv.id, "dismiss"));
        const dismiss = sessionActionButton(
          "dismiss",
          "-",
          `Dismiss ${title}`,
          false,
          actionDisabled || dismissPending,
          false
        );
        dismiss.onclick = (ev) => {
          ev.stopPropagation();
          dismissSession(srv.id);
        };
        right.appendChild(dismiss);
        if (srv.id === activeServerId && activeAction === "dismiss") focusAfterRender = dismiss;
      }
    }
    const closeVerb = isOwned(srv) ? "Close" : "Forget";
    const close = el("button", `close${isClosing ? " busy" : ""}`, isClosing ? "" : "×");
    close.type = "button";
    close.disabled = isClosing;
    close.title = isClosing
      ? `${closeVerb === "Close" ? "Closing" : "Forgetting"} server`
      : `${closeVerb} server`;
    close.setAttribute("aria-label", `${close.title} ${title}`);
    close.onclick = (ev) => { ev.stopPropagation(); closeServer(srv.id); };
    right.appendChild(close);
    row.appendChild(right);

    railEl.appendChild(row);
    if (srv.id === activeServerId && !focusAfterRender) focusAfterRender = activeWasClose ? close : row;
  }

  if (focusAfterRender) focusAfterRender.focus({ preventScroll: true });
  if (paletteOpen) renderPalette();
}

function focusServerRow(id) {
  requestAnimationFrame(() => {
    const row = Array.from(railEl.querySelectorAll(".srv")).find((item) => item.dataset.serverId === id);
    if (row) row.focus({ preventScroll: true });
  });
}

function activateServer(id) {
  if (!id) return;
  selectServer(id);
  focusServerRow(id);
}

function jumpNextUnread() {
  const target = nextAttentionCandidate();
  if (!target) {
    showHostStatus({ level: "info", source: "rail", message: "no unread sessions" });
    return;
  }
  activateServer(target.srv.id);
}

function openPalette(query = "") {
  if (!paletteEl || !paletteInput || !paletteList || !displayed().length) return;
  paletteOpen = true;
  paletteQuery = query;
  paletteIndex = 0;
  paletteEl.classList.remove("hidden");
  paletteInput.value = paletteQuery;
  renderPalette();
  requestAnimationFrame(() => {
    paletteInput.focus({ preventScroll: true });
    paletteInput.select();
  });
}

function closePalette() {
  if (!paletteOpen) return;
  paletteOpen = false;
  paletteQuery = "";
  paletteIndex = 0;
  if (paletteEl) paletteEl.classList.add("hidden");
  if (paletteList) paletteList.replaceChildren();
  focusServerRow(selected);
}

function renderPalette() {
  if (!paletteOpen || !paletteList) return;
  const candidates = paletteCandidates();
  if (paletteIndex >= candidates.length) paletteIndex = Math.max(0, candidates.length - 1);
  paletteList.replaceChildren();

  if (!candidates.length) {
    const empty = el("div", "palette-empty", "No matches");
    paletteList.appendChild(empty);
    return;
  }

  candidates.slice(0, 12).forEach((candidate, index) => {
    const active = index === paletteIndex;
    const item = el("button", `palette-item${active ? " active" : ""}`, "");
    item.type = "button";
    item.setAttribute("role", "option");
    item.setAttribute("aria-selected", active ? "true" : "false");
    item.onclick = () => choosePalette(candidate.srv.id);

    const glyph = el("span", "palette-glyph", STATE_GLYPH[candidate.state] || "·");
    const body = el("span", "palette-body");
    body.appendChild(el("span", "palette-title", candidate.title));
    body.appendChild(el("span", "palette-sub", candidate.preview));
    const meta = el("span", "palette-meta");
    if (candidate.agent && candidate.agent.unread) meta.appendChild(el("span", "dot tiny"));
    if (candidate.attention) meta.appendChild(el("span", "badge palette-badge", waitingAge(candidate.agent && candidate.agent.waiting_since)));
    if (candidate.srv.id === selected) meta.appendChild(el("span", "state-chip selected-chip", "open"));

    item.appendChild(glyph);
    item.appendChild(body);
    item.appendChild(meta);
    paletteList.appendChild(item);
  });
}

function choosePalette(id) {
  closePalette();
  activateServer(id);
}

function movePalette(delta) {
  const count = paletteCandidates().slice(0, 12).length;
  if (!count) return;
  paletteIndex = (paletteIndex + delta + count) % count;
  renderPalette();
}

function showHostStatus(payload) {
  if (statusTimer) clearTimeout(statusTimer);
  if (!payload || !payload.message) {
    statusOverride = null;
    render();
    return;
  }
  statusOverride = {
    level: payload.level || "error",
    source: payload.source || "host",
    message: payload.message,
  };
  render();
  const message = statusOverride.message;
  const level = statusOverride.level || "error";
  statusTimer = setTimeout(() => {
    if (statusOverride && statusOverride.message === message) {
      clearStatusOverride();
    }
  }, statusClearDelay(level));
}

async function setSessionMuted(id, muted) {
  const key = actionKey(id, "mute");
  if (sessionActions.has(key)) return;
  sessionActions.add(key);
  render();
  try {
    await invoke("set_session_muted", { sessionId: id, muted });
  } catch (e) {
    showHostStatus({
      level: "error",
      source: "rail",
      message: `${muted ? "mute" : "unmute"} failed: ${String(e)}`,
    });
  } finally {
    sessionActions.delete(key);
    render();
  }
}

async function setSessionSoloed(id, soloed) {
  const key = actionKey(id, "solo");
  if (sessionActions.has(key)) return;
  sessionActions.add(key);
  render();
  try {
    await invoke("set_session_soloed", { sessionId: id, soloed });
  } catch (e) {
    showHostStatus({
      level: "error",
      source: "rail",
      message: `${soloed ? "solo" : "clear solo"} failed: ${String(e)}`,
    });
  } finally {
    sessionActions.delete(key);
    render();
  }
}

async function dismissSession(id) {
  const key = actionKey(id, "dismiss");
  if (sessionActions.has(key)) return;
  sessionActions.add(key);
  render();
  try {
    await invoke("dismiss_session", { sessionId: id });
  } catch (e) {
    showHostStatus({
      level: "error",
      source: "rail",
      message: `dismiss failed: ${String(e)}`,
    });
  } finally {
    sessionActions.delete(key);
    render();
  }
}

async function spawnServer() {
  if (spawning) return;
  spawning = true;
  statusOverride = null;
  render();
  try {
    const id = await invoke("spawn_server");
    pending = pending.filter((p) => p.id !== id);
    pending.push({ id, label: id, owned: true, startedAt: Date.now() });
    selectServer(id); // shows the loading page + selects the pending tab
  } catch (e) {
    showHostStatus({ level: "error", source: "rail", message: `spawn failed: ${String(e)}` });
  } finally {
    spawning = false;
    render();
  }
}

async function closeServer(id) {
  if (closing.has(id)) return;
  const target = displayed().find((srv) => srv.id === id);
  const owned = isOwned(target);
  const wasPending = pending.some((p) => p.id === id);
  closing.add(id);
  render();
  try {
    const killed = await invoke("close_server", { id });
    pending = pending.filter((p) => p.id !== id);
    if (!killed && !wasPending && owned) {
      showHostStatus({
        level: "warning",
        source: "rail",
        message: "server removed; process was not Fleet-owned",
      });
    }
    await refreshServers();
  } catch (e) {
    showHostStatus({ level: "error", source: "rail", message: `close failed: ${String(e)}` });
  } finally {
    closing.delete(id);
    render();
  }
}

async function selectServer(id) {
  selected = id;
  render();
  try { await invoke("select_server", { id }); } catch (e) { /* ignore */ }
}

function handleRowKeydown(ev, row, id) {
  if (ev.target && ev.target.closest && ev.target.closest("button")) return;
  if (ev.key === "Enter" || ev.key === " ") {
    ev.preventDefault();
    selectServer(id);
  } else if (ev.key === "Backspace" || ev.key === "Delete") {
    ev.preventDefault();
    closeServer(id);
  } else if (ev.key === "ArrowDown") {
    ev.preventDefault();
    focusAdjacent(row, 1);
  } else if (ev.key === "ArrowUp") {
    ev.preventDefault();
    focusAdjacent(row, -1);
  }
}

function focusAdjacent(row, delta) {
  const rows = Array.from(railEl.querySelectorAll(".srv"));
  const index = rows.indexOf(row);
  if (index < 0 || !rows.length) return;
  rows[(index + delta + rows.length) % rows.length].focus();
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

async function init() {
  await listen("servers-changed", () => refreshServers());
  await listen("inbox", (e) => { inbox = e.payload || inbox; render(); });
  await listen("host-status", (e) => showHostStatus(e.payload));
  try {
    inbox = await invoke("get_inbox");
  } catch (e) {
    inbox = { tabs: [], waiting_count: 0, connected: false };
  }
  try {
    showHostStatus(await invoke("get_host_status"));
  } catch (e) {
    // ignore
  }
  await refreshServers();
  render();
}

init();
setInterval(render, 1000); // tick the "waiting Nm" age

if (paletteInput) {
  paletteInput.addEventListener("input", () => {
    paletteQuery = paletteInput.value;
    paletteIndex = 0;
    renderPalette();
  });
  paletteInput.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      ev.preventDefault();
      closePalette();
    } else if (ev.key === "ArrowDown") {
      ev.preventDefault();
      movePalette(1);
    } else if (ev.key === "ArrowUp") {
      ev.preventDefault();
      movePalette(-1);
    } else if (ev.key === "Enter") {
      ev.preventDefault();
      const choice = paletteCandidates().slice(0, 12)[paletteIndex];
      if (choice) choosePalette(choice.srv.id);
    }
  });
}

if (paletteEl) {
  paletteEl.addEventListener("mousedown", (ev) => {
    if (ev.target === paletteEl) closePalette();
  });
}

document.addEventListener("keydown", (ev) => {
  const command = ev.metaKey || ev.ctrlKey;
  if (command && ev.key.toLowerCase() === "k") {
    ev.preventDefault();
    if (paletteOpen) closePalette();
    else openPalette();
  } else if (command && ev.key.toLowerCase() === "j") {
    ev.preventDefault();
    jumpNextUnread();
  } else if (command && ev.shiftKey && ev.key.toLowerCase() === "n") {
    ev.preventDefault();
    spawnServer();
  } else if (ev.key === "Escape" && paletteOpen) {
    ev.preventDefault();
    closePalette();
  } else if (ev.key === "Escape" && statusOverride) {
    clearStatusOverride();
  }
});
