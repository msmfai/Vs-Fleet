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
const rowMenuEl = document.getElementById("row-menu");
const createMenuEl = document.getElementById("create-menu");
const promptEl = document.getElementById("prompt");
const promptMessageEl = document.getElementById("prompt-message");
const promptInput = document.getElementById("prompt-input");
const promptOkBtn = document.getElementById("prompt-ok");
const promptCancelBtn = document.getElementById("prompt-cancel");
if (spawnBtn) spawnBtn.onclick = toggleCreateMenu;
if (jumpBtn) jumpBtn.onclick = jumpNextUnread;
if (paletteBtn) paletteBtn.onclick = () => openPalette();

const STATE_GLYPH = { working: "▶", waiting: "⏸", idle: "·", done: "✓", error: "✕", dead: "☠" };
const AGENT_GLYPH = { claude: "C", codex: "O", agent: "A" };
const LOCATION_GLYPH = { laptop: "⌨", docker: "▣", remote: "⇄" };
const URGENCY_LABEL = { approval: "approval", question: "question", "idle-done": "done" };
const PENDING_SLOW_MS = 15_000;
const PENDING_TIMEOUT_MS = 45_000;
const STATUS_CLEAR_MS = 8_000;
const STATUS_CLEAR_BY_LEVEL_MS = { error: 30_000, warning: 20_000, info: 10_000 };

let servers = [];          // registered (phoned home) — from the backend
let pending = [];          // [{id, label, startedAt}] spawned but not yet registered
let selected = null;
let desiredSelection = null;
let desiredAcknowledge = true;
let inbox = { tabs: [], waiting_count: 0, waiting_total: 0, connected: false };
let inboxRevision = 0;
let refreshGeneration = 0;
let statusOverride = null;
let statusTimer = null;
let spawning = false;
const closing = new Set();
const sessionActions = new Set();
let paletteOpen = false;
let paletteQuery = "";
let paletteIndex = 0;
let rowMenu = { open: false, serverId: null, x: 0, y: 0, index: 0 };
let createMenu = { open: false, x: 0, y: 0 };

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

// In-DOM replacement for window.prompt(). macOS WKWebView (which Tauri uses)
// returns null from the native window.prompt without ever showing a dialog, so
// any flow relying on it silently dies — this is why rename/open-folder did
// nothing. This overlay works in every webview. Resolves to the entered string,
// or null if cancelled. Lives outside #rail, so rail re-renders don't disturb it.
let promptResolve = null;
function closePrompt(value) {
  if (!promptEl) return;
  promptEl.classList.add("hidden");
  promptInput.onkeydown = null;
  const resolve = promptResolve;
  promptResolve = null;
  if (resolve) resolve(value);
}
function domPrompt(message, defaultValue = "") {
  return new Promise((resolve) => {
    if (!promptEl) {
      resolve(null);
      return;
    }
    // If a prompt is already open, cancel it before opening the next.
    if (promptResolve) closePrompt(null);
    promptResolve = resolve;
    promptMessageEl.textContent = message;
    promptInput.value = defaultValue;
    promptEl.classList.remove("hidden");
    promptInput.focus();
    promptInput.select();
    promptInput.onkeydown = (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        ev.stopPropagation();
        closePrompt(promptInput.value);
      } else if (ev.key === "Escape") {
        ev.preventDefault();
        ev.stopPropagation();
        closePrompt(null);
      }
    };
  });
}
if (promptOkBtn) promptOkBtn.onclick = () => closePrompt(promptInput.value);
if (promptCancelBtn) promptCancelBtn.onclick = () => closePrompt(null);
// Click on the dim backdrop (outside the panel) cancels, like the palette.
if (promptEl) {
  promptEl.onclick = (ev) => {
    if (ev.target === promptEl) closePrompt(null);
  };
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

function canCloseServerRow(srv) {
  return Boolean(srv && !srv.agentOnly);
}

function canRenameServerRow(srv) {
  return Boolean(srv && !srv.agentOnly);
}

function serverById(id) {
  return displayed().find((srv) => srv.id === id);
}

function clearLocalSelection(id) {
  if (selected === id) selected = null;
  if (desiredSelection === id) {
    desiredSelection = null;
    desiredAcknowledge = true;
  }
}

function removeInboxSession(id) {
  const oldNotify = notifyMapForTabs(inbox.tabs || []);
  const tabs = (inbox.tabs || []).filter((tab) => tab.session_id !== id);
  replaceInboxTabs(tabs, oldNotify);
}

function setInbox(next) {
  inbox = next || inbox;
  inboxRevision += 1;
}

function shouldNotifyTab(tab, anySoloed) {
  return Boolean(tab.attention && !tab.muted && (!anySoloed || tab.soloed));
}

function notifyMapForTabs(tabs) {
  const anySoloed = tabs.some((tab) => tab.soloed);
  return new Map(tabs.map((tab) => [tab.session_id, shouldNotifyTab(tab, anySoloed)]));
}

function reconcileUnread(tab, oldNotify, newNotify) {
  if (!oldNotify && newNotify && !tab.unread) return true;
  if (oldNotify && !newNotify && tab.unread) return false;
  return Boolean(tab.unread);
}

function deriveInboxTabs(tabs, oldNotify = null) {
  const anySoloed = tabs.some((tab) => tab.soloed);
  const nextTabs = tabs.map((tab) => {
    const nextNotify = shouldNotifyTab(tab, anySoloed);
    const previousNotify = oldNotify ? Boolean(oldNotify.get(tab.session_id)) : nextNotify;
    return {
      ...tab,
      unread: reconcileUnread(tab, previousNotify, nextNotify),
      ping_suppressed: Boolean(tab.attention && !nextNotify),
      pinging: nextNotify,
    };
  });
  return {
    tabs: nextTabs,
    waiting_count: nextTabs.filter((tab) => tab.pinging).length,
    waiting_total: nextTabs.filter((tab) => tab.attention).length,
  };
}

function replaceInboxTabs(tabs, oldNotify = null) {
  setInbox({ ...inbox, ...deriveInboxTabs(tabs, oldNotify) });
}

function updateInboxTabs(mapper) {
  const tabs = inbox.tabs || [];
  replaceInboxTabs(tabs.map(mapper), notifyMapForTabs(tabs));
}

function applyLocalMute(id, muted) {
  updateInboxTabs((tab) => tab.session_id === id
    ? { ...tab, muted, soloed: false }
    : tab);
}

function applyLocalSolo(id, soloed) {
  updateInboxTabs((tab) => {
    if (soloed) {
      return tab.session_id === id
        ? { ...tab, soloed: true, muted: false }
        : { ...tab, soloed: false };
    }
    return tab.session_id === id
      ? { ...tab, soloed: false, muted: false }
      : tab;
  });
}

function applyLocalFocus(id) {
  updateInboxTabs((tab) => tab.session_id === id
    ? { ...tab, unread: false }
    : tab);
}

function rowTitle(srv, agent) {
  // A user rename pins the label: show it verbatim instead of letting the
  // agent/session-derived title override it (otherwise a reporter phone-home
  // clobbers the rename a moment later).
  if (srv.renamed) return displayLabel(srv);
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

function canForgetAgentOnly(srv, agent) {
  return Boolean(srv && srv.agentOnly && agent);
}

function canRetryServer(srv, pendingState) {
  return Boolean(srv && srv.pending && isOwned(srv) && pendingState && pendingState.state === "error");
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
    pinging: agent ? Boolean(agent.pinging) : false,
    suppressed: agent ? Boolean(agent.ping_suppressed) : false,
    flags: rowFlags(srv, agent),
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
  if (model.pinging) score += 20;
  else if (model.attention) score += 5;
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

function renderedRows() {
  return displayed().map(serverRowModel);
}

function unreadCandidates(options = {}) {
  return renderedRows().filter((row) => {
    if (!row.agent || !row.agent.unread) return false;
    if (options.openableOnly && row.srv.agentOnly) return false;
    return true;
  });
}

function nextUnreadCandidate(options = {}) {
  const candidates = unreadCandidates(options);
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
  if (agent.ping_suppressed) return ["silenced"];
  return [];
}

function rowFlags(srv, agent) {
  return stateFlags(agent);
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

function appendIdentityChips(parent, agent) {
  if (!agent) return;
  const wrap = el("span", "identity");
  const agentToken = token(agent.agent);
  const locationToken = token(agent.location);
  if (agentToken) {
    const chip = el("span", `identity-chip agent-${agentToken}`, AGENT_GLYPH[agentToken] || agentToken[0].toUpperCase());
    chip.title = agentToken;
    wrap.appendChild(chip);
  }
  if (locationToken) {
    const chip = el("span", `identity-chip location-${locationToken}`, LOCATION_GLYPH[locationToken] || locationToken[0].toUpperCase());
    chip.title = locationToken;
    wrap.appendChild(chip);
  }
  if (wrap.childNodes.length) parent.appendChild(wrap);
}

function actionKey(id, action) {
  return `${action}:${id}`;
}

function sessionActionBusy(id) {
  for (const action of ["mute", "solo", "dismiss", "retry", "focus"]) {
    if (sessionActions.has(actionKey(id, action))) return true;
  }
  return false;
}

function rowMenuItems(id) {
  const srv = displayed().find((item) => item.id === id);
  if (!srv) return [];
  const model = serverRowModel(srv);
  const busy = sessionActionBusy(id);
  const closeVerb = isOwned(srv) ? "Close" : "Forget";
  const items = [{
    id: "open",
    label: model.srv.agentOnly ? "Show Session" : "Open",
    action: () => activateServer(id),
  }];
  items.push({
    id: "copy-id",
    label: "Copy Session ID",
    action: () => copyRowValue("session id", id),
  });
  if (canRenameServerRow(model.srv)) {
    items.push({
      id: "rename",
      label: "Rename",
      action: () => renameRow(id),
    });
  }
  if (model.srv.url) {
    items.push({
      id: "copy-url",
      label: "Copy URL",
      action: () => copyRowValue("url", model.srv.url),
    });
    items.push({
      id: "open-browser",
      label: "Open in Browser",
      action: () => openRowInBrowser(id),
    });
  }

  if (model.agent) {
    items.push({
      id: "mute",
      label: model.agent.muted ? "Unmute" : "Mute",
      disabled: busy || !inbox.connected,
      action: () => toggleMuteRow(id),
    });
    items.push({
      id: "solo",
      label: model.agent.soloed ? "Clear Alert Focus" : "Alert Focus",
      disabled: busy || !inbox.connected,
      action: () => toggleSoloRow(id),
    });
  }

  if (canRetryServer(model.srv, model.pendingState)) {
    items.push({
      id: "retry",
      label: "Retry",
      disabled: busy || spawning,
      action: () => retryRow(id),
    });
  }

  if (canDismissAgent(model.agent, model.state)) {
    items.push({
      id: "dismiss",
      label: "Dismiss",
      disabled: busy || !inbox.connected,
      action: () => dismissRow(id),
    });
  } else if (canForgetAgentOnly(model.srv, model.agent)) {
    items.push({
      id: "forget-session",
      label: "Forget Session",
      disabled: busy || !inbox.connected,
      action: () => forgetAgentOnlyRow(id),
    });
  }

  if (canCloseServerRow(srv)) {
    items.push({
      id: "close",
      label: closeVerb,
      disabled: busy || closing.has(id),
      action: () => closeServer(id),
    });
  }

  return items;
}

function visibleRowMenuItems() {
  return rowMenu.open && rowMenu.serverId ? rowMenuItems(rowMenu.serverId) : [];
}

// Registered servers + still-pending ones + Hub sessions whose editor bridge has
// not registered yet. The rail is an inbox first; bridge lag must not hide state.
function displayed() {
  const regIds = new Set(servers.map((s) => s.id));
  const stillPending = pending.filter((p) => !regIds.has(p.id)).map((p) => ({ ...p, pending: true, agentOnly: false }));
  const visibleIds = new Set([...regIds, ...stillPending.map((p) => p.id)]);
  const inboxOnly = (inbox.tabs || [])
    .filter((tab) => !visibleIds.has(tab.session_id))
    .map((tab) => ({
      id: tab.session_id,
      label: tab.title || tab.session_id,
      url: "",
      owned: false,
      pending: false,
      agentOnly: true,
    }));
  const tabOrder = new Map((inbox.tabs || []).map((tab, index) => [tab.session_id, index]));
  return [...servers.map((s) => ({ ...s, pending: false, agentOnly: false })), ...stillPending, ...inboxOnly]
    .map((srv, index) => ({
      srv,
      index,
      rank: tabOrder.has(srv.id) ? tabOrder.get(srv.id) : 10_000 + index,
    }))
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .map((entry) => entry.srv);
}

function silencedWaitingCount() {
  return (inbox.tabs || []).filter((tab) => tab.attention && tab.ping_suppressed).length;
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
  const silenced = silencedWaitingCount();
  if (silenced > 0) {
    return {
      message: silenced === 1 ? "1 muted" : `${silenced} muted`,
      level: "info",
      title: "Waiting sessions silenced by alert settings",
    };
  }
  return { message: "connected", level: "connected", title: "" };
}

function updateSpawnButton() {
  if (!spawnBtn) return;
  spawnBtn.disabled = spawning;
  spawnBtn.classList.toggle("busy", spawning);
  spawnBtn.title = spawning ? "Starting server" : "New server";
  spawnBtn.setAttribute("aria-label", spawning ? "Starting server" : "New server");
  spawnBtn.setAttribute("aria-busy", spawning ? "true" : "false");
}

function countBadge(count) {
  if (count <= 0) return null;
  return count > 9 ? "9+" : String(count);
}

function countPhrase(count, noun) {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

function setToolButtonState(btn, options) {
  if (!btn) return;
  btn.disabled = Boolean(options.disabled);
  btn.title = options.title;
  btn.setAttribute("aria-label", options.title);
  btn.classList.toggle("attention", Boolean(options.attention));
  const count = countBadge(options.count || 0);
  if (count) btn.dataset.count = count;
  else delete btn.dataset.count;
}

function updateToolbarButtons() {
  const unread = unreadCandidates();
  const openableUnread = unreadCandidates({ openableOnly: true });
  const rowCount = displayed().length;
  const waitingOnBridge = unread.length > 0 && openableUnread.length === 0;
  setToolButtonState(jumpBtn, {
    disabled: !openableUnread.length,
    attention: Boolean(openableUnread.length),
    count: openableUnread.length,
    title: openableUnread.length
      ? `Open next unread session (${countPhrase(openableUnread.length, "session")})`
      : waitingOnBridge ? "Unread sessions are still connecting to editors" : "No unread sessions",
  });
  setToolButtonState(paletteBtn, {
    disabled: !rowCount,
    attention: false,
    count: 0,
    title: rowCount ? `Session palette (${countPhrase(rowCount, "session")})` : "No sessions",
  });
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

function emptyTitle() {
  if (spawning) return "Starting server";
  if (statusOverride && statusOverride.message) return statusOverride.message;
  if (!inbox.connected) return "Hub disconnected";
  return "No sessions";
}

function renderEmptyState(status) {
  railEl.removeAttribute("role");
  railEl.removeAttribute("aria-activedescendant");
  const wrap = el("div", `empty-state ${status.level}`);
  wrap.setAttribute("role", "status");

  const marker = el("span", spawning ? "empty-spinner" : "empty-dot", "");
  const title = el("div", "empty-title", emptyTitle());
  const action = el("button", `empty-action${spawning ? " busy" : ""}`, spawning ? "Starting" : "New Server");
  action.type = "button";
  action.disabled = spawning;
  action.title = spawning ? "Starting server" : "New Server";
  action.setAttribute("aria-label", action.title);
  action.setAttribute("aria-busy", spawning ? "true" : "false");
  action.onclick = spawnServer;

  wrap.appendChild(marker);
  wrap.appendChild(title);
  wrap.appendChild(action);
  railEl.appendChild(wrap);
}

function render() {
  const list = displayed();
  if (rowMenu.open && rowMenu.serverId && !list.some((srv) => srv.id === rowMenu.serverId)) {
    closeRowMenu();
  }
  const status = railStatus(list);
  statusEl.textContent = status.message;
  statusEl.title = status.title || "";
  statusEl.className = `status ${status.level}`;
  railEl.setAttribute("aria-busy", spawning ? "true" : "false");
  updateSpawnButton();
  updateToolbarButtons();
  renderStatusDetail();

  railEl.replaceChildren();
  if (!list.length) {
    if (paletteOpen) closePalette();
    closeRowMenu();
    renderEmptyState(status);
    return;
  }
  railEl.setAttribute("role", "list");

  for (const model of list.map(serverRowModel)) {
    const { srv, agent: a, pendingState, isClosing, title, state, attention, pinging, suppressed, flags, preview } = model;
    const actionBusy = sessionActionBusy(srv.id);

    const row = el(
      "div",
      `srv ${state}${pinging ? " attention" : ""}${flags.includes("muted") ? " muted-state" : ""}${suppressed ? " suppressed-state" : ""}${flags.includes("solo") ? " soloed-state" : ""}${srv.id === selected ? " selected" : ""}${srv.pending ? " pending" : ""}${srv.agentOnly ? " agent-only" : ""}${pendingState && pendingState.level ? ` ${pendingState.level}` : ""}${isClosing ? " closing" : ""}${actionBusy ? " action-pending" : ""}`
    );
    row.dataset.serverId = srv.id;
    row.setAttribute("role", "listitem");
    row.setAttribute("aria-current", srv.id === selected ? "true" : "false");
    row.setAttribute("aria-busy", actionBusy ? "true" : "false");
    row.setAttribute(
      "aria-label",
      [title, state, preview].filter(Boolean).join(", ")
    );
    row.onclick = () => activateServer(srv.id);
    row.oncontextmenu = (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      openRowMenu(srv.id, ev.clientX, ev.clientY);
    };

    if (srv.pending && pendingState.state !== "error") row.appendChild(el("span", "glyph spinner", ""));
    else row.appendChild(el("span", "glyph", STATE_GLYPH[state] || "·"));

    const body = el("div", "body");
    body.appendChild(el("div", "label", title));
    const sub = el("div", "sub");
    appendIdentityChips(sub, a);
    if (isClosing) sub.appendChild(el("span", "preview muted", "closing…"));
    else if (pendingState) sub.appendChild(el("span", `preview ${pendingState.state === "error" ? "error-text" : "muted"}`, pendingState.text));
    else sub.appendChild(el("span", `preview ${a && a.last_message && inbox.connected ? "" : "muted"}`, preview));
    body.appendChild(sub);
    row.appendChild(body);

    const right = el("div", "right");
    if (pinging) appendAttentionBadges(right, a);
    else if (a && a.unread && !a.ping_suppressed) right.appendChild(el("span", "dot"));
    row.appendChild(right);

    railEl.appendChild(row);
  }

  if (paletteOpen) renderPalette();
  if (rowMenu.open) renderRowMenu();
  if (createMenu.open) renderCreateMenu();
}

async function activateServer(id, options = {}) {
  if (!id) return;
  const target = serverById(id);
  if (target && target.agentOnly) {
    selected = id;
    desiredSelection = id;
    desiredAcknowledge = options.acknowledge !== false;
    render();
    showHostStatus({
      level: "info",
      source: "rail",
      message: "session is visible; editor is still connecting",
    });
    return;
  }

  const selectedOk = await selectServer(id);
  if (selectedOk && options.acknowledge !== false && agentFor(id)) focusSession(id);
}

function jumpNextUnread() {
  const target = nextUnreadCandidate({ openableOnly: true });
  if (!target) {
    const message = unreadCandidates().length
      ? "unread sessions are still connecting to editors"
      : "no unread sessions";
    showHostStatus({ level: "info", source: "rail", message });
    return;
  }
  activateServer(target.srv.id);
}

function cycleUnread() {
  const target = nextUnreadCandidate();
  if (!target) {
    showHostStatus({ level: "info", source: "rail", message: "no unread sessions" });
    return;
  }
  activateServer(target.srv.id, { acknowledge: false });
}

function openPalette(query = "") {
  if (!paletteEl || !paletteInput || !paletteList) return;
  if (!displayed().length) {
    showHostStatus({
      level: "info",
      source: "palette",
      message: spawning ? "server is still starting" : "no sessions",
    });
    return;
  }
  paletteOpen = true;
  paletteQuery = query;
  paletteIndex = 0;
  paletteEl.classList.remove("hidden");
  paletteInput.value = paletteQuery;
  paletteInput.setAttribute("aria-expanded", "true");
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
  if (paletteInput) {
    paletteInput.setAttribute("aria-expanded", "false");
    paletteInput.removeAttribute("aria-activedescendant");
  }
}

function renderPalette() {
  if (!paletteOpen || !paletteList) return;
  const candidates = paletteCandidates();
  const visible = candidates.slice(0, 12);
  if (paletteIndex >= visible.length) paletteIndex = Math.max(0, visible.length - 1);
  paletteList.replaceChildren();

  if (!visible.length) {
    if (paletteInput) paletteInput.removeAttribute("aria-activedescendant");
    const empty = el("div", "palette-empty", "No matches");
    paletteList.appendChild(empty);
    return;
  }

  visible.forEach((candidate, index) => {
    const active = index === paletteIndex;
    const item = el("div", `palette-item${active ? " active" : ""}`, "");
    const optionId = `palette-option-${index}`;
    item.id = optionId;
    item.setAttribute("role", "option");
    item.setAttribute("aria-selected", active ? "true" : "false");
    if (active && paletteInput) paletteInput.setAttribute("aria-activedescendant", optionId);
    item.onmouseenter = () => {
      if (paletteIndex === index) return;
      paletteIndex = index;
      renderPalette();
    };
    item.onmousedown = (ev) => ev.preventDefault();
    item.onclick = () => choosePalette(candidate.srv.id);

    const glyph = el("span", "palette-glyph", STATE_GLYPH[candidate.state] || "·");
    const body = el("span", "palette-body");
    body.appendChild(el("span", "palette-title", candidate.title));
    body.appendChild(el("span", "palette-sub", candidate.preview));
    const meta = el("span", "palette-meta");
    if (candidate.pinging) meta.appendChild(el("span", "badge palette-badge", waitingAge(candidate.agent && candidate.agent.waiting_since)));
    else if (candidate.agent && candidate.agent.unread && !candidate.agent.ping_suppressed) meta.appendChild(el("span", "dot tiny"));
    item.appendChild(glyph);
    item.appendChild(body);
    item.appendChild(meta);
    paletteList.appendChild(item);
    if (active) item.scrollIntoView({ block: "nearest" });
  });
}

function choosePalette(id) {
  closePalette();
  activateServer(id);
}

function createMenuItem(label, action, options = {}) {
  const item = el("button", "row-menu-item", "");
  item.type = "button";
  item.setAttribute("role", "menuitem");
  item.disabled = Boolean(options.disabled);
  item.appendChild(el("span", "", label));
  item.onclick = () => {
    if (item.disabled) return;
    closeCreateMenu();
    action();
  };
  return item;
}

function renderCreateMenu() {
  if (!createMenuEl || !createMenu.open) return;
  createMenuEl.replaceChildren();
  createMenuEl.classList.remove("hidden");
  createMenuEl.style.left = `${createMenu.x}px`;
  createMenuEl.style.top = `${createMenu.y}px`;

  createMenuEl.appendChild(createMenuItem("New in Home", () => spawnServer({ mode: "local" })));
  createMenuEl.appendChild(createMenuItem("Open Folder...", openFolderPrompt));

  const rect = createMenuEl.getBoundingClientRect();
  const left = Math.max(8, Math.min(createMenu.x, window.innerWidth - rect.width - 8));
  const top = Math.max(8, Math.min(createMenu.y, window.innerHeight - rect.height - 8));
  createMenuEl.style.left = `${left}px`;
  createMenuEl.style.top = `${top}px`;
}

function openCreateMenu() {
  if (!createMenuEl || !spawnBtn || spawning) return;
  if (paletteOpen) closePalette();
  closeRowMenu();
  const rect = spawnBtn.getBoundingClientRect();
  createMenu = {
    ...createMenu,
    open: true,
    x: rect.right - 180,
    y: rect.bottom + 5,
  };
  renderCreateMenu();
}

function closeCreateMenu() {
  createMenu.open = false;
  if (!createMenuEl) return;
  createMenuEl.classList.add("hidden");
  createMenuEl.replaceChildren();
}

function toggleCreateMenu(ev) {
  if (ev) ev.stopPropagation();
  if (createMenu.open) closeCreateMenu();
  else openCreateMenu();
}

async function openFolderPrompt() {
  const folder = await domPrompt("Open folder path", "~");
  if (!folder || !folder.trim()) return;
  spawnServer({ mode: "local", folder: folder.trim() });
}

function openRowMenu(id, x, y) {
  if (!rowMenuEl) return;
  if (paletteOpen) closePalette();
  closeCreateMenu();
  rowMenu = {
    open: true,
    serverId: id,
    x,
    y,
    index: 0,
  };
  renderRowMenu();
}

function closeRowMenu() {
  if (!rowMenu.open) return;
  rowMenu = { open: false, serverId: null, x: 0, y: 0, index: 0 };
  if (rowMenuEl) {
    rowMenuEl.classList.add("hidden");
    rowMenuEl.replaceChildren();
    rowMenuEl.removeAttribute("aria-activedescendant");
  }
}

function positionRowMenu() {
  if (!rowMenuEl || !rowMenu.open) return;
  const rect = rowMenuEl.getBoundingClientRect();
  const left = Math.max(8, Math.min(rowMenu.x, window.innerWidth - rect.width - 8));
  const top = Math.max(8, Math.min(rowMenu.y, window.innerHeight - rect.height - 8));
  rowMenuEl.style.left = `${left}px`;
  rowMenuEl.style.top = `${top}px`;
}

function renderRowMenu() {
  if (!rowMenuEl || !rowMenu.open) return;
  const items = visibleRowMenuItems();
  if (!items.length) {
    closeRowMenu();
    return;
  }
  if (rowMenu.index >= items.length) rowMenu.index = Math.max(0, items.length - 1);
  rowMenuEl.replaceChildren();
  rowMenuEl.classList.remove("hidden");
  rowMenuEl.style.left = `${rowMenu.x}px`;
  rowMenuEl.style.top = `${rowMenu.y}px`;

  items.forEach((item, index) => {
    const active = index === rowMenu.index;
    const btn = el("button", `row-menu-item${active ? " active" : ""}${item.disabled ? " disabled" : ""}`, "");
    const itemId = `row-menu-${item.id}`;
    btn.id = itemId;
    btn.type = "button";
    btn.setAttribute("role", "menuitem");
    if (item.disabled) btn.setAttribute("aria-disabled", "true");
    btn.onmouseenter = () => {
      if (rowMenu.index === index) return;
      rowMenu.index = index;
      renderRowMenu();
    };
    btn.onmousedown = (ev) => ev.preventDefault();
    btn.onclick = () => chooseRowMenuItem(index);
    btn.appendChild(el("span", "row-menu-label", item.label));
    rowMenuEl.appendChild(btn);
  });

  requestAnimationFrame(() => positionRowMenu());
}

function chooseRowMenuItem(index = rowMenu.index) {
  const item = visibleRowMenuItems()[index];
  if (!item || item.disabled) return;
  const action = item.action;
  closeRowMenu();
  action();
}

async function copyRowValue(label, value) {
  try {
    await copyText(String(value || ""));
    showRailInfo(`copied ${label}`);
  } catch (e) {
    showHostStatus({
      level: "error",
      source: "rail",
      message: `copy failed: ${String(e)}`,
    });
  }
}

function applyLocalServerLabel(id, label) {
  servers = servers.map((srv) => srv.id === id ? { ...srv, label, renamed: true } : srv);
  pending = pending.map((srv) => srv.id === id ? { ...srv, label, renamed: true } : srv);
}

async function renameRow(id) {
  const srv = serverById(id);
  if (!canRenameServerRow(srv)) {
    showRailInfo("nothing to rename");
    return;
  }
  const current = displayLabel(srv);
  const next = await domPrompt("Rename session", current);
  if (next == null) return;
  const label = next.trim();
  if (!label) {
    showRailInfo("label cannot be empty");
    return;
  }
  if (label === current) return;
  try {
    const saved = await invoke("rename_server", { id, label });
    applyLocalServerLabel(id, saved);
    showRailInfo("renamed");
    await refreshServers();
  } catch (e) {
    showHostStatus({
      level: "error",
      source: "rail",
      message: `rename failed: ${String(e)}`,
    });
  }
}

async function openRowInBrowser(id) {
  const srv = serverById(id);
  if (!srv || !srv.url) {
    showRailInfo("server URL unavailable");
    return;
  }
  try {
    await invoke("open_server_external", { id });
    showRailInfo("opened in browser");
  } catch (e) {
    showHostStatus({
      level: "error",
      source: "rail",
      message: `open browser failed: ${String(e)}`,
    });
  }
}

async function copyText(text) {
  if (!text) throw new Error("nothing to copy");
  if (navigator.clipboard && navigator.clipboard.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.left = "-9999px";
  document.body.appendChild(textarea);
  textarea.select();
  try {
    if (!document.execCommand("copy")) throw new Error("clipboard unavailable");
  } finally {
    textarea.remove();
  }
}

function showHostStatus(payload) {
  if (statusTimer) clearTimeout(statusTimer);
  statusTimer = null;
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

function isRecoverableStatus(status) {
  if (!status || !status.message) return false;
  const message = status.message.toLowerCase();
  return (
    message === "session is visible; editor is still connecting"
    || message === "server is not ready to open"
    || message.includes("editor still connecting")
    || message.startsWith("open failed:")
  );
}

function clearRecoveredStatus() {
  if (!isRecoverableStatus(statusOverride)) return;
  clearStatusOverride();
}

async function setSessionMuted(id, muted) {
  const key = actionKey(id, "mute");
  if (sessionActions.has(key)) return;
  const previousInbox = inbox;
  sessionActions.add(key);
  applyLocalMute(id, muted);
  const optimisticRevision = inboxRevision;
  render();
  try {
    await invoke("set_session_muted", { sessionId: id, muted });
  } catch (e) {
    if (inboxRevision === optimisticRevision) setInbox(previousInbox);
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
  const previousInbox = inbox;
  sessionActions.add(key);
  applyLocalSolo(id, soloed);
  const optimisticRevision = inboxRevision;
  render();
  try {
    await invoke("set_session_soloed", { sessionId: id, soloed });
  } catch (e) {
    if (inboxRevision === optimisticRevision) setInbox(previousInbox);
    showHostStatus({
      level: "error",
      source: "rail",
      message: `${soloed ? "alert focus" : "clear alert focus"} failed: ${String(e)}`,
    });
  } finally {
    sessionActions.delete(key);
    render();
  }
}

async function dismissSession(id) {
  const key = actionKey(id, "dismiss");
  if (sessionActions.has(key)) return;
  const target = serverById(id);
  sessionActions.add(key);
  render();
  try {
    await invoke("dismiss_session", { sessionId: id });
    removeInboxSession(id);
    if (target && target.agentOnly) clearLocalSelection(id);
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

async function focusSession(id) {
  if (!inbox.connected) return;
  const key = actionKey(id, "focus");
  if (sessionActions.has(key)) return;
  const previousInbox = inbox;
  sessionActions.add(key);
  applyLocalFocus(id);
  const optimisticRevision = inboxRevision;
  render();
  try {
    await invoke("focus_session", { sessionId: id });
  } catch (e) {
    if (inboxRevision === optimisticRevision) setInbox(previousInbox);
    showHostStatus({
      level: "warning",
      source: "rail",
      message: `focus ack failed: ${String(e)}`,
    });
  } finally {
    sessionActions.delete(key);
    render();
  }
}

async function spawnServer(request = {}) {
  if (spawning) return;
  closeCreateMenu();
  spawning = true;
  statusOverride = null;
  render();
  try {
    const id = await invoke("spawn_server_with_options", { request });
    pending = pending.filter((p) => p.id !== id);
    pending.push({ id, label: id, owned: true, startedAt: Date.now() });
    await selectServer(id); // shows the loading page + selects the pending tab
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
    clearLocalSelection(id);
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

async function retryServer(id) {
  const key = actionKey(id, "retry");
  if (sessionActions.has(key) || spawning) return;
  sessionActions.add(key);
  closing.add(id);
  pending = pending.filter((p) => p.id !== id);
  render();
  try {
    await invoke("close_server", { id });
    clearLocalSelection(id);
    closing.delete(id);
    await refreshServers();
    await spawnServer();
  } catch (e) {
    showHostStatus({ level: "error", source: "rail", message: `retry failed: ${String(e)}` });
  } finally {
    closing.delete(id);
    sessionActions.delete(key);
    render();
  }
}

async function selectServer(id) {
  const previousSelected = selected;
  const previousDesired = desiredSelection;
  const previousDesiredAcknowledge = desiredAcknowledge;
  selected = id;
  // A successful editor open fulfils any deferred agent-only target. If opening
  // fails, restore it so bridge registration can still complete the selection.
  desiredSelection = null;
  desiredAcknowledge = true;
  render();
  try {
    const opened = await invoke("select_server", { id });
    if (opened) {
      clearRecoveredStatus();
      return true;
    }
    selected = previousSelected;
    desiredSelection = previousDesired;
    desiredAcknowledge = previousDesiredAcknowledge;
    showHostStatus({ level: "warning", source: "rail", message: "server is not ready to open" });
    return false;
  } catch (e) {
    selected = previousSelected;
    desiredSelection = previousDesired;
    desiredAcknowledge = previousDesiredAcknowledge;
    showHostStatus({ level: "error", source: "rail", message: `open failed: ${String(e)}` });
    return false;
  } finally {
    render();
  }
}

function showRailInfo(message) {
  showHostStatus({ level: "info", source: "rail", message });
}

function toggleMuteRow(id) {
  if (sessionActionBusy(id)) {
    showRailInfo("action in progress");
    return;
  }
  const agent = agentFor(id);
  if (!agent) {
    showRailInfo("no agent state to mute");
    return;
  }
  if (!inbox.connected) {
    showRailInfo("hub disconnected");
    return;
  }
  setSessionMuted(id, !agent.muted);
}

function toggleSoloRow(id) {
  if (sessionActionBusy(id)) {
    showRailInfo("action in progress");
    return;
  }
  const agent = agentFor(id);
  if (!agent) {
    showRailInfo("no alert state for this session");
    return;
  }
  if (!inbox.connected) {
    showRailInfo("hub disconnected");
    return;
  }
  setSessionSoloed(id, !agent.soloed);
}

function retryRow(id) {
  if (sessionActionBusy(id) || spawning) {
    showRailInfo("action in progress");
    return;
  }
  const srv = displayed().find((item) => item.id === id);
  const pendingState = srv && srv.pending ? pendingVisual(srv) : null;
  if (!canRetryServer(srv, pendingState)) {
    showRailInfo("nothing to retry");
    return;
  }
  retryServer(id);
}

function dismissRow(id) {
  if (sessionActionBusy(id)) {
    showRailInfo("action in progress");
    return;
  }
  const agent = agentFor(id);
  if (!canDismissAgent(agent, agent && agent.state)) {
    showRailInfo("nothing to dismiss");
    return;
  }
  if (!inbox.connected) {
    showRailInfo("hub disconnected");
    return;
  }
  dismissSession(id);
}

function forgetAgentOnlyRow(id) {
  if (sessionActionBusy(id)) {
    showRailInfo("action in progress");
    return;
  }
  const srv = displayed().find((item) => item.id === id);
  const agent = agentFor(id);
  if (!canForgetAgentOnly(srv, agent)) {
    showRailInfo("nothing to forget");
    return;
  }
  if (!inbox.connected) {
    showRailInfo("hub disconnected");
    return;
  }
  dismissSession(id);
}

function removeRow(id) {
  if (sessionActionBusy(id)) {
    showHostStatus({ level: "info", source: "rail", message: "action in progress" });
    return;
  }
  const srv = displayed().find((item) => item.id === id);
  const agent = agentFor(id);
  if (srv && canCloseServerRow(srv)) {
    closeServer(id);
  } else if (canDismissAgent(agent, agent && agent.state)) {
    dismissSession(id);
  } else if (canForgetAgentOnly(srv, agent)) {
    forgetAgentOnlyRow(id);
  } else {
    showHostStatus({ level: "info", source: "rail", message: "no server process to close" });
  }
}

async function refreshServers() {
  const generation = ++refreshGeneration;
  let nextServers = [];
  let backendSelected = null;
  try {
    nextServers = await invoke("get_servers");
    backendSelected = await invoke("selected_server");
  } catch (e) {
    nextServers = [];
  }
  if (generation !== refreshGeneration) return;
  servers = nextServers;

  // Drop pending entries that have now phoned home.
  pending = pending.filter((p) => !servers.some((s) => s.id === p.id));

  const ids = displayed().map((s) => s.id);
  const preferred = desiredSelection || selected;
  const desired = desiredSelection;
  const acknowledgeDesired = desiredAcknowledge;
  const backendValid = backendSelected && ids.includes(backendSelected);
  selected = preferred && ids.includes(preferred) ? preferred : (backendValid ? backendSelected : null);

  // Auto-select the first server once any exist and nothing valid is selected.
  if ((!selected || !ids.includes(selected)) && servers.length) {
    desiredSelection = null;
    desiredAcknowledge = true;
    await selectServer(servers[0].id);
    return;
  }
  const selectedRegistered = selected && servers.some((s) => s.id === selected);
  const selectionNeedsOpen = selectedRegistered && (desired === selected || backendSelected !== selected);
  // If a deferred target just became registered, navigate to it. Otherwise avoid
  // re-selecting the already-active editor on routine server-list refreshes.
  if (selectionNeedsOpen) {
    const selectedOk = await selectServer(selected);
    if (selectedOk && desired === selected && acknowledgeDesired && agentFor(selected)) focusSession(selected);
    return;
  }
  render();
}

// Called from Rust (mux::select) so the rail highlight stays in sync.
window.__fleetSyncSelection = async () => {
  try {
    selected = await invoke("selected_server");
    desiredSelection = null;
    desiredAcknowledge = true;
    if (selected) clearRecoveredStatus();
    render();
  } catch (e) { /* ignore */ }
};

window.__fleetOpenPalette = () => openPalette();
window.__fleetJumpNextUnread = () => jumpNextUnread();
window.__fleetCycleUnread = () => cycleUnread();

async function init() {
  await listen("servers-changed", () => refreshServers());
  await listen("inbox", (e) => { setInbox(e.payload || inbox); render(); });
  await listen("host-status", (e) => showHostStatus(e.payload));
  try {
    setInbox(await invoke("get_inbox"));
  } catch (e) {
    setInbox({ tabs: [], waiting_count: 0, waiting_total: 0, connected: false });
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
}

if (paletteEl) {
  paletteEl.addEventListener("mousedown", (ev) => {
    if (ev.target === paletteEl) closePalette();
  });
}

document.addEventListener("mousedown", (ev) => {
  if (createMenu.open && createMenuEl && !createMenuEl.contains(ev.target) && ev.target !== spawnBtn) {
    closeCreateMenu();
  }
  if (!rowMenu.open || !rowMenuEl) return;
  if (!rowMenuEl.contains(ev.target)) closeRowMenu();
});

window.addEventListener("resize", () => {
  if (rowMenu.open) closeRowMenu();
  if (createMenu.open) closeCreateMenu();
});
