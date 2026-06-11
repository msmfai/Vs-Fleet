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
const cycleBtn = document.getElementById("cycle-unread");
const paletteBtn = document.getElementById("palette-open");
const paletteEl = document.getElementById("palette");
const paletteInput = document.getElementById("palette-input");
const paletteList = document.getElementById("palette-list");
const rowMenuEl = document.getElementById("row-menu");
if (spawnBtn) spawnBtn.onclick = spawnServer;
if (jumpBtn) jumpBtn.onclick = jumpNextUnread;
if (cycleBtn) cycleBtn.onclick = cycleUnread;
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
let statusOverride = null;
let statusTimer = null;
let spawning = false;
const closing = new Set();
const sessionActions = new Set();
let paletteOpen = false;
let paletteQuery = "";
let paletteIndex = 0;
let paletteRestoreEl = null;
let rowMenu = { open: false, serverId: null, x: 0, y: 0, index: 0, restoreEl: null };

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

function canCloseServerRow(srv) {
  return Boolean(srv && !srv.agentOnly);
}

function serverById(id) {
  return displayed().find((srv) => srv.id === id);
}

function focusedServerId() {
  const active = document.activeElement;
  return active && active.closest ? active.closest(".srv")?.dataset.serverId || null : null;
}

function adjacentDisplayId(id) {
  const list = displayed();
  const index = list.findIndex((srv) => srv.id === id);
  if (index < 0 || list.length <= 1) return null;
  const nextIndex = index < list.length - 1 ? index + 1 : index - 1;
  return list[nextIndex]?.id || null;
}

function focusBestServerRow(...ids) {
  const visible = new Set(displayed().map((srv) => srv.id));
  const target = ids.find((id) => id && visible.has(id)) || displayed()[0]?.id;
  if (target) focusServerRow(target);
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

function bridgeState(srv) {
  return srv && srv.agentOnly ? "editor bridge not connected" : "";
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
    bridgeState(model.srv),
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
  const flags = stateFlags(agent);
  if (srv && srv.agentOnly) flags.push("bridge");
  return flags;
}

function stateFlagLabel(flag) {
  if (flag === "bridge") return "bridge";
  return flag === "silenced" ? "silent" : flag;
}

function stateFlagTitle(flag) {
  if (flag === "bridge") return "editor bridge not connected";
  return stateFlagLabel(flag);
}

function appendStateFlagChips(parent, flags, extraClass = "") {
  for (const flag of flags) {
    const chip = el("span", `state-chip ${flag}${extraClass ? ` ${extraClass}` : ""}`, stateFlagLabel(flag));
    chip.title = stateFlagTitle(flag);
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

function rowKeyboardShortcuts(model) {
  const shortcuts = ["Enter", "Space", "ContextMenu", "Shift+F10", "ArrowDown", "ArrowUp", "Home", "End"];
  if (sessionActionBusy(model.srv.id)) return shortcuts.join(" ");
  if (model.agent && inbox.connected) shortcuts.push("M", "S");
  if (model.srv.url) shortcuts.push("B");
  if (canRetryServer(model.srv, model.pendingState)) shortcuts.push("R");
  if (canDismissAgent(model.agent, model.state) || canForgetAgentOnly(model.srv, model.agent)) {
    shortcuts.push("D");
  }
  if (
    canCloseServerRow(model.srv)
    || canDismissAgent(model.agent, model.state)
    || canForgetAgentOnly(model.srv, model.agent)
  ) {
    shortcuts.push("Delete", "Backspace");
  }
  return shortcuts.join(" ");
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
    shortcut: "Enter",
    action: () => activateServer(id),
  }];
  items.push({
    id: "copy-id",
    label: "Copy Session ID",
    shortcut: "I",
    action: () => copyRowValue("session id", id),
  });
  if (model.srv.url) {
    items.push({
      id: "copy-url",
      label: "Copy URL",
      shortcut: "U",
      action: () => copyRowValue("url", model.srv.url),
    });
    items.push({
      id: "open-browser",
      label: "Open in Browser",
      shortcut: "B",
      action: () => openRowInBrowser(id),
    });
  }

  if (model.agent) {
    items.push({
      id: "mute",
      label: model.agent.muted ? "Unmute" : "Mute",
      shortcut: "M",
      disabled: busy || !inbox.connected,
      action: () => toggleMuteRow(id),
    });
    items.push({
      id: "solo",
      label: model.agent.soloed ? "Clear Solo" : "Solo",
      shortcut: "S",
      disabled: busy || !inbox.connected,
      action: () => toggleSoloRow(id),
    });
  }

  if (canRetryServer(model.srv, model.pendingState)) {
    items.push({
      id: "retry",
      label: "Retry",
      shortcut: "R",
      disabled: busy || spawning,
      action: () => retryRow(id),
    });
  }

  if (canDismissAgent(model.agent, model.state)) {
    items.push({
      id: "dismiss",
      label: "Dismiss",
      shortcut: "D",
      disabled: busy || !inbox.connected,
      action: () => dismissRow(id),
    });
  } else if (canForgetAgentOnly(model.srv, model.agent)) {
    items.push({
      id: "forget-session",
      label: "Forget Session",
      shortcut: "D",
      disabled: busy || !inbox.connected,
      action: () => forgetAgentOnlyRow(id),
    });
  }

  if (canCloseServerRow(srv)) {
    items.push({
      id: "close",
      label: closeVerb,
      shortcut: "Delete",
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
      title: "Waiting sessions silenced by mute/solo",
    };
  }
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
      ? `Jump to next unread (${countPhrase(openableUnread.length, "session")})`
      : waitingOnBridge ? "Unread sessions are waiting for editor bridges" : "No unread sessions",
  });
  setToolButtonState(cycleBtn, {
    disabled: !unread.length,
    attention: Boolean(unread.length),
    count: unread.length,
    title: unread.length
      ? `Cycle unread without marking read (${countPhrase(unread.length, "session")})`
      : "No unread sessions",
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
    closeRowMenu({ restoreFocus: false });
  }
  const status = railStatus(list);
  statusEl.textContent = status.message;
  statusEl.title = status.title || "";
  statusEl.className = `status ${status.level}`;
  railEl.setAttribute("aria-busy", spawning ? "true" : "false");
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
  if (!list.length) {
    if (paletteOpen) closePalette({ restoreFocus: false });
    closeRowMenu({ restoreFocus: false });
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
    row.tabIndex = 0;
    row.setAttribute("role", "listitem");
    row.setAttribute("aria-current", srv.id === selected ? "true" : "false");
    row.setAttribute("aria-busy", actionBusy ? "true" : "false");
    row.setAttribute(
      "aria-label",
      [title, state, preview, ...flags.map(stateFlagTitle)].filter(Boolean).join(", ")
    );
    row.setAttribute("aria-keyshortcuts", rowKeyboardShortcuts(model));
    if (srv.agentOnly) row.title = bridgeState(srv);
    row.onclick = () => activateServer(srv.id);
    row.onkeydown = (ev) => handleRowKeydown(ev, row, srv.id);
    row.oncontextmenu = (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      openRowMenu(srv.id, ev.clientX, ev.clientY, row);
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
    appendStateFlagChips(right, flags);
    if (pinging) appendAttentionBadges(right, a);
    else if (a && a.unread && !a.ping_suppressed) right.appendChild(el("span", "dot"));

    const actions = el("button", "row-actions", "...");
    actions.type = "button";
    actions.dataset.action = "menu";
    actions.title = `Actions for ${title}`;
    actions.setAttribute("aria-label", actions.title);
    actions.setAttribute("aria-haspopup", "menu");
    actions.onclick = (ev) => {
      ev.stopPropagation();
      const rect = actions.getBoundingClientRect();
      openRowMenu(srv.id, rect.right - 4, rect.bottom + 4, actions);
    };
    right.appendChild(actions);
    if (srv.id === activeServerId && activeAction === "menu") focusAfterRender = actions;

    if (a) {
      const mutePending = sessionActions.has(actionKey(srv.id, "mute"));
      const soloPending = sessionActions.has(actionKey(srv.id, "solo"));
      const actionDisabled = !inbox.connected || actionBusy;
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
    if (canRetryServer(srv, pendingState)) {
      const retryPending = sessionActions.has(actionKey(srv.id, "retry")) || spawning;
      const retry = sessionActionButton(
        "retry",
        "↻",
        `Retry ${title}`,
        false,
        retryPending || actionBusy,
        false
      );
      retry.onclick = (ev) => {
        ev.stopPropagation();
        retryServer(srv.id);
      };
      right.appendChild(retry);
      if (srv.id === activeServerId && activeAction === "retry") focusAfterRender = retry;
    }
    let close = null;
    if (canCloseServerRow(srv)) {
      const closeVerb = isOwned(srv) ? "Close" : "Forget";
      close = el("button", `close${isClosing ? " busy" : ""}`, isClosing ? "" : "×");
      close.type = "button";
      close.disabled = isClosing || actionBusy;
      close.title = isClosing
        ? `${closeVerb === "Close" ? "Closing" : "Forgetting"} server`
        : actionBusy ? "Action in progress"
        : `${closeVerb} server`;
      close.setAttribute("aria-label", `${close.title} ${title}`);
      close.onclick = (ev) => { ev.stopPropagation(); closeServer(srv.id); };
      right.appendChild(close);
    }
    row.appendChild(right);

    railEl.appendChild(row);
    if (srv.id === activeServerId && !focusAfterRender) focusAfterRender = activeWasClose && close ? close : row;
  }

  if (focusAfterRender) focusAfterRender.focus({ preventScroll: true });
  if (paletteOpen) renderPalette();
  if (rowMenu.open) renderRowMenu();
}

function focusServerRow(id) {
  requestAnimationFrame(() => {
    const row = Array.from(railEl.querySelectorAll(".srv")).find((item) => item.dataset.serverId === id);
    if (row) row.focus({ preventScroll: true });
  });
}

async function activateServer(id, options = {}) {
  if (!id) return;
  const target = serverById(id);
  if (target && target.agentOnly) {
    selected = id;
    desiredSelection = id;
    desiredAcknowledge = options.acknowledge !== false;
    render();
    focusServerRow(id);
    showHostStatus({
      level: "info",
      source: "rail",
      message: "session visible; editor bridge not connected yet",
    });
    return;
  }

  const selectedOk = await selectServer(id);
  if (options.keepRailFocus) focusServerRow(id);
  if (selectedOk && options.acknowledge !== false && agentFor(id)) focusSession(id);
}

function jumpNextUnread() {
  const target = nextUnreadCandidate({ openableOnly: true });
  if (!target) {
    const message = unreadCandidates().length
      ? "unread sessions are waiting for editor bridges"
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
  const active = document.activeElement;
  paletteRestoreEl = active && active !== document.body && active.focus ? active : null;
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

function closePalette(options = {}) {
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
  const restoreEl = paletteRestoreEl;
  paletteRestoreEl = null;
  if (options.restoreFocus !== false) {
    if (restoreEl && restoreEl.isConnected && restoreEl.focus) restoreEl.focus({ preventScroll: true });
    else focusServerRow(selected);
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
    item.tabIndex = -1;
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
    if (candidate.flags.length) appendStateFlagChips(meta, candidate.flags, "palette-state");
    if (candidate.srv.id === selected) meta.appendChild(el("span", "state-chip selected-chip", "open"));

    item.appendChild(glyph);
    item.appendChild(body);
    item.appendChild(meta);
    paletteList.appendChild(item);
    if (active) item.scrollIntoView({ block: "nearest" });
  });
}

function choosePalette(id) {
  closePalette({ restoreFocus: false });
  activateServer(id);
}

function movePalette(delta) {
  const count = paletteCandidates().slice(0, 12).length;
  if (!count) return;
  paletteIndex = (paletteIndex + delta + count) % count;
  renderPalette();
}

function openRowMenu(id, x, y, restoreEl = null) {
  if (!rowMenuEl) return;
  if (paletteOpen) closePalette({ restoreFocus: false });
  rowMenu = {
    open: true,
    serverId: id,
    x,
    y,
    index: 0,
    restoreEl: restoreEl || document.activeElement,
  };
  renderRowMenu();
  requestAnimationFrame(() => focusRowMenuItem());
}

function closeRowMenu(options = {}) {
  if (!rowMenu.open) return;
  const restoreEl = rowMenu.restoreEl;
  rowMenu = { open: false, serverId: null, x: 0, y: 0, index: 0, restoreEl: null };
  if (rowMenuEl) {
    rowMenuEl.classList.add("hidden");
    rowMenuEl.replaceChildren();
    rowMenuEl.removeAttribute("aria-activedescendant");
  }
  if (options.restoreFocus !== false) {
    if (restoreEl && restoreEl.isConnected && restoreEl.focus) restoreEl.focus({ preventScroll: true });
    else focusServerRow(selected);
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

function focusRowMenuItem() {
  if (!rowMenuEl || !rowMenu.open) return;
  const item = rowMenuEl.querySelector(".row-menu-item.active");
  if (item) item.focus({ preventScroll: true });
}

function renderRowMenu() {
  if (!rowMenuEl || !rowMenu.open) return;
  const items = visibleRowMenuItems();
  if (!items.length) {
    closeRowMenu({ restoreFocus: false });
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
    btn.tabIndex = active ? 0 : -1;
    btn.setAttribute("role", "menuitem");
    if (item.disabled) btn.setAttribute("aria-disabled", "true");
    if (item.shortcut) btn.setAttribute("aria-keyshortcuts", item.shortcut);
    btn.onmouseenter = () => {
      if (rowMenu.index === index) return;
      rowMenu.index = index;
      renderRowMenu();
    };
    btn.onmousedown = (ev) => ev.preventDefault();
    btn.onclick = () => chooseRowMenuItem(index);
    btn.appendChild(el("span", "row-menu-label", item.label));
    if (item.shortcut) btn.appendChild(el("span", "row-menu-shortcut", item.shortcut));
    rowMenuEl.appendChild(btn);
    if (active) rowMenuEl.setAttribute("aria-activedescendant", itemId);
  });

  requestAnimationFrame(() => {
    positionRowMenu();
    focusRowMenuItem();
  });
}

function moveRowMenu(delta) {
  const items = visibleRowMenuItems();
  if (!items.length) return;
  rowMenu.index = (rowMenu.index + delta + items.length) % items.length;
  renderRowMenu();
}

function chooseRowMenuItem(index = rowMenu.index) {
  const item = visibleRowMenuItems()[index];
  if (!item || item.disabled) return;
  const action = item.action;
  closeRowMenu({ restoreFocus: false });
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
    message === "session visible; editor bridge not connected yet"
    || message === "server is not ready to open"
    || message.includes("editor bridge not connected")
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
  const target = serverById(id);
  const shouldRestoreFocus = focusedServerId() === id;
  const focusFallback = adjacentDisplayId(id);
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
    if (shouldRestoreFocus) {
      focusBestServerRow(target && target.agentOnly ? focusFallback : selected, focusFallback);
    }
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

async function spawnServer() {
  if (spawning) return;
  spawning = true;
  statusOverride = null;
  render();
  try {
    const id = await invoke("spawn_server");
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
  const shouldRestoreFocus = focusedServerId() === id;
  const focusFallback = adjacentDisplayId(id);
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
    if (shouldRestoreFocus) focusBestServerRow(selected, focusFallback);
  }
}

async function retryServer(id) {
  const key = actionKey(id, "retry");
  if (sessionActions.has(key) || spawning) return;
  const shouldRestoreFocus = focusedServerId() === id;
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
    if (shouldRestoreFocus) focusBestServerRow(selected);
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

function handleRowKeydown(ev, row, id) {
  if (ev.target && ev.target.closest && ev.target.closest("button")) return;
  const plainKey = ev.metaKey || ev.ctrlKey || ev.altKey ? "" : ev.key.toLowerCase();
  if (ev.key === "Enter" || ev.key === " ") {
    ev.preventDefault();
    activateServer(id);
  } else if (ev.key === "ContextMenu" || (ev.shiftKey && ev.key === "F10")) {
    ev.preventDefault();
    const rect = row.getBoundingClientRect();
    openRowMenu(id, rect.left + 24, rect.top + Math.min(rect.height - 4, 32), row);
  } else if (plainKey === "m") {
    ev.preventDefault();
    toggleMuteRow(id);
  } else if (plainKey === "s") {
    ev.preventDefault();
    toggleSoloRow(id);
  } else if (plainKey === "b") {
    ev.preventDefault();
    openRowInBrowser(id);
  } else if (plainKey === "r") {
    ev.preventDefault();
    retryRow(id);
  } else if (plainKey === "d") {
    ev.preventDefault();
    dismissRow(id);
  } else if (ev.key === "Backspace" || ev.key === "Delete") {
    ev.preventDefault();
    removeRow(id);
  } else if (ev.key === "ArrowDown") {
    ev.preventDefault();
    focusAdjacent(row, 1);
  } else if (ev.key === "ArrowUp") {
    ev.preventDefault();
    focusAdjacent(row, -1);
  } else if (ev.key === "Home") {
    ev.preventDefault();
    focusBoundary("first");
  } else if (ev.key === "End") {
    ev.preventDefault();
    focusBoundary("last");
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
    showRailInfo("no agent state to solo");
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

function focusAdjacent(row, delta) {
  const rows = serverRows();
  const index = rows.indexOf(row);
  if (index < 0 || !rows.length) return;
  rows[(index + delta + rows.length) % rows.length].focus();
}

function focusBoundary(edge) {
  const rows = serverRows();
  const target = edge === "last" ? rows[rows.length - 1] : rows[0];
  if (target) target.focus();
}

function serverRows() {
  return Array.from(railEl.querySelectorAll(".srv"));
}

async function refreshServers() {
  let backendSelected = null;
  try {
    servers = await invoke("get_servers");
    backendSelected = await invoke("selected_server");
  } catch (e) {
    servers = [];
  }
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
  const handlePaletteKey = (ev) => {
    ev.preventDefault();
    ev.stopPropagation();
  };

  paletteInput.addEventListener("input", () => {
    paletteQuery = paletteInput.value;
    paletteIndex = 0;
    renderPalette();
  });
  paletteInput.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      handlePaletteKey(ev);
      closePalette();
    } else if (ev.key === "ArrowDown") {
      handlePaletteKey(ev);
      movePalette(1);
    } else if (ev.key === "ArrowUp") {
      handlePaletteKey(ev);
      movePalette(-1);
    } else if (ev.key === "Enter") {
      handlePaletteKey(ev);
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

if (rowMenuEl) {
  rowMenuEl.addEventListener("keydown", (ev) => {
    ev.stopPropagation();
    const key = ev.key.toLowerCase();
    if (ev.key === "Escape") {
      ev.preventDefault();
      closeRowMenu();
    } else if (ev.key === "ArrowDown") {
      ev.preventDefault();
      moveRowMenu(1);
    } else if (ev.key === "ArrowUp") {
      ev.preventDefault();
      moveRowMenu(-1);
    } else if (ev.key === "Home") {
      ev.preventDefault();
      rowMenu.index = 0;
      renderRowMenu();
    } else if (ev.key === "End") {
      ev.preventDefault();
      rowMenu.index = Math.max(0, visibleRowMenuItems().length - 1);
      renderRowMenu();
    } else if (ev.key === "Enter" || ev.key === " ") {
      ev.preventDefault();
      chooseRowMenuItem();
    } else if (!ev.metaKey && !ev.ctrlKey && !ev.altKey) {
      const index = visibleRowMenuItems().findIndex((item) => token(item.shortcut) === key);
      if (index >= 0) {
        ev.preventDefault();
        chooseRowMenuItem(index);
      }
    }
  });
}

document.addEventListener("mousedown", (ev) => {
  if (!rowMenu.open || !rowMenuEl) return;
  if (!rowMenuEl.contains(ev.target)) closeRowMenu({ restoreFocus: false });
});

window.addEventListener("resize", () => {
  if (rowMenu.open) closeRowMenu({ restoreFocus: false });
});

document.addEventListener("keydown", (ev) => {
  const command = ev.metaKey || ev.ctrlKey;
  const key = ev.key.toLowerCase();
  if (rowMenu.open) {
    if (ev.key === "Escape") {
      ev.preventDefault();
      closeRowMenu();
    }
    return;
  }
  if (paletteOpen) {
    if (command && key === "k") {
      ev.preventDefault();
      closePalette();
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      closePalette();
    } else if (command && (key === "j" || (ev.shiftKey && key === "n"))) {
      ev.preventDefault();
    }
    return;
  }

  if (command && key === "k") {
    ev.preventDefault();
    openPalette();
  } else if (command && ev.shiftKey && key === "j") {
    ev.preventDefault();
    cycleUnread();
  } else if (command && key === "j") {
    ev.preventDefault();
    jumpNextUnread();
  } else if (command && ev.shiftKey && key === "n") {
    ev.preventDefault();
    spawnServer();
  } else if (ev.key === "Escape" && statusOverride) {
    clearStatusOverride();
  }
});
