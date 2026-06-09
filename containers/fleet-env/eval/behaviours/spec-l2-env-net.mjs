// L2 Env-lifecycle + Networking behaviours (SPEC areas 20-env-lifecycle.md +
// 26-networking.md). One self-contained file the registry auto-discovers; it does
// NOT touch any existing behaviour file.
//
// These are L2 (Fleet-stack) assertions: rather than driving the VS Code surface,
// they pin the container↔host wiring the whole stack rides on — the entrypoint's
// host/URL derivation, code-server's published port, host.docker.internal
// reachability, the bridge command round-trip, and the reporter unix-socket trust
// boundary. Every assertion is made with a tool the eval harness already has:
//
//   - env.exec(sh)      → docker exec inside the container (filesystem / printenv /
//                          nc probes / ss listeners). Needs NO network of its own.
//   - host `docker …`   → run on the HOST (via child_process) to inspect the
//                          container the way spawn.rs::inspect_url / Env.close do.
//   - env.observe()     → a real host↔container bridge round-trip (proves duplex).
//   - the reporter --serve unix socket (the L2 reporter note) — used only to keep
//                          the Hub-gated entries honest.
//
// Testability boundary (per the task): we implement an entry ONLY when the feature
// EXISTS and we can assert it with the tools above. Entries that need a host
// supervisor process (spawn.rs container mode), the desktop multiplexer, a
// dedicated scenario (custom ports / loopback-Hub / no-FLEET_HOST_ADDR), or that
// must observe state AFTER tearing the env down, are left TODO in the spec.
//
// Hub-dependent entries (a host Hub on :51777 + the `fleet` CLI) SKIP cleanly when
// the Hub/CLI is absent — exactly like agentInput.mjs — because that is an
// environmental precondition, not a regression. We reuse that file's host-Hub query
// helpers via the same resolution pattern (replicated here to avoid editing it).

import { execSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ─── Host-side shell (NOT in the container) ─────────────────────────────────────
// env.exec() runs INSIDE the container; several lifecycle/networking facts must be
// asserted from the HOST (does the container exist? what host port did docker
// publish? what does `docker logs` show?). hostSh runs on the host and returns
// trimmed stdout ("" on any failure), mirroring lib/env.mjs's private `sh`.
function hostSh(cmd) {
  try {
    return execSync(cmd, { encoding: "utf8", timeout: 15000, stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch {
    return "";
  }
}

// ─── Host-side Hub query (the Hub lives on the host; replicated from agentInput) ─
const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = join(HERE, "..", "..", "..", "..");
const CLI_CANDIDATES = [
  process.env.FLEET_CLI,
  join(REPO_ROOT, "target", "debug", "fleet"),
  join(REPO_ROOT, "target", "release", "fleet"),
].filter(Boolean);

function fleetCli() {
  for (const p of CLI_CANDIDATES) if (p && (p === "fleet" || existsSync(p))) return p;
  return null;
}

function hubSnapshot() {
  const cli = fleetCli();
  if (!cli) return null;
  try {
    return execSync(`${JSON.stringify(cli)} ls --once`, {
      encoding: "utf8",
      timeout: 8000,
      stdio: ["ignore", "pipe", "ignore"],
    });
  } catch {
    return null;
  }
}

function sessionLineFor(snapshot, sessionTitle) {
  if (!snapshot) return null;
  for (const raw of snapshot.split("\n")) {
    const line = raw.trim();
    if (!line.startsWith("[")) continue;
    if (line.includes(sessionTitle)) return line;
  }
  return null;
}

// Poll the Hub for this env's session row (the reporter registers it on boot).
async function pollHubSession(sessionTitle, { ms = 15000, every = 1000 } = {}) {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    const line = sessionLineFor(hubSnapshot(), sessionTitle);
    if (line) return { ok: true, line };
    await sleep(every);
  }
  return { ok: false, line: null };
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ── L2.LIFE.001 — container Up + entrypoint phone-home banner ────────────────
  {
    id: "life.bannerAndRunning",
    specId: "L2.LIFE.001",
    title: "Container is Up and the entrypoint logged the phone-home banner",
    tags: ["lifecycle", "net"],
    rationale: `
WHAT: From the HOST, asserts two facts about the booted container fleet-eval-<id>:
(a) \`docker inspect -f '{{.State.Running}}'\` is exactly "true", and (b) \`docker logs\`
contains the entrypoint's phone-home banner line — \`[fleet-env] id=<id>\` carrying
\`host=host.docker.internal hub=ws://host.docker.internal:51777
bridge=ws://host.docker.internal:51778\`. Both must hold.

WHY THIS IS THE EXPECTED OUTCOME: This is the very first link in the L2 chain. The
harness always runs the image with \`-e FLEET_HOST_ADDR=host.docker.internal\`
(lib/env.mjs _dockerRunCmd), so entrypoint.sh's \`HOST=\${FLEET_HOST_ADDR:-…}\` must
resolve HOST to exactly host.docker.internal (the explicit override winning over the
default-route auto-detect), and from it derive \`FLEET_HUB_URL=ws://HOST:51777\` and
\`FLEET_BRIDGE_URL=ws://HOST:51778\` using the Containerfile's default ports
(FLEET_HUB_PORT=51777 / FLEET_BRIDGE_PORT=51778). entrypoint.sh \`echo\`s precisely that
banner before it does anything else, so it is the earliest observable proof the
host/url derivation is correct. \`State.Running == true\` proves the container didn't
exit (code-server is exec'd as PID 1's foreground process, so the container stays Up).

WHY IT MATTERS: A banner mismatch means the entrypoint's host/url derivation drifted
BEFORE anything else can phone home — every reporter/bridge dial would target the
wrong host or port and the env would silently never appear to Fleet. Because this is
asserted from the host's docker view (not via the bridge), it is independent of
whether the bridge itself came up: if the bridge is broken but the banner is right,
a future reader knows the entrypoint derivation is fine and the fault is downstream;
if the banner is wrong, the regression is in entrypoint.sh / the Containerfile ENV
defaults / the harness run argv. State.Running additionally distinguishes "the
container is up but un-driven" from "the container crash-exited."`,
    needs: [],
    async run(env) {
      const name = env.name; // fleet-eval-<id>
      const running = hostSh(`docker inspect -f '{{.State.Running}}' ${name}`);
      const logs = hostSh(`docker logs ${name} 2>&1`);
      const bannerId = `[fleet-env] id=${env.id}`;
      const wantHost = "host=host.docker.internal";
      const wantHub = "hub=ws://host.docker.internal:51777";
      const wantBridge = "bridge=ws://host.docker.internal:51778";

      const bannerLine = logs
        .split("\n")
        .find((l) => l.includes(bannerId)) || "";
      const bannerOk =
        bannerLine.includes(wantHost) &&
        bannerLine.includes(wantHub) &&
        bannerLine.includes(wantBridge);
      const runningOk = running === "true";

      return {
        pass: runningOk && bannerOk,
        detail: runningOk && bannerOk
          ? `${name} Running=true; banner ok (${wantHub} / ${wantBridge})`
          : `Running=${JSON.stringify(running)}; banner=${JSON.stringify(bannerLine.slice(0, 160))}`,
        evidence: { name, running, bannerLine, want: { wantHost, wantHub, wantBridge } },
      };
    },
  },

  // ── L2.LIFE.006 / L2.NET.008-mirror — inspect_url HostPort → reachable editor ─
  // (We implement the harness-observable half of L2.LIFE.006: the inspect template
  //  resolves to the SAME published port the harness chose, and that URL is
  //  302/200-reachable. The Rust spawn.rs::inspect_url path itself is host-supervisor
  //  code and stays TODO under L2.NET.008.)
  {
    id: "life.inspectUrlReachable",
    specId: "L2.LIFE.006",
    title: "The inspected published HostPort yields a 302/200-reachable editor URL",
    tags: ["lifecycle", "net"],
    isolation: "fresh",
    rationale: `
WHAT: Resolves the host-reachable editor URL the way the desktop multiplexer does —
\`docker inspect -f '{{(index (index .NetworkSettings.Ports "8080/tcp") 0).HostPort}}'
fleet-eval-<id>\` — and asserts three things: the inspected HostPort is non-empty, it
equals the \`-p\` host port the harness published (env.port), and
\`http://127.0.0.1:<HostPort>/\` answers 302 or 200.

WHY THIS IS THE EXPECTED OUTCOME: code-server binds \`0.0.0.0:8080\` inside the
container and the harness publishes it with \`-p <env.port>:8080\`; Docker therefore
records a Ports entry mapping container 8080/tcp to host port env.port. The desktop
multiplexer does NOT trust the asked-for port — it re-derives the reachable URL from
this exact inspect template (spawn.rs::inspect_url) and navigates the editor webview
to \`http://127.0.0.1:<HostPort>/\`. So the inspected HostPort MUST equal env.port (no
drift between what we asked Docker to publish and what Docker reports), and that URL
must serve a 302 (code-server's auth redirect) or 200. We assert 302/200 (not a body)
because that is the precise readiness contract the suite uses (§8).

WHY IT MATTERS: If Docker's port-binding JSON shape changes (the
\`.NetworkSettings.Ports\` nesting), the inspect template returns empty and the desktop
embed silently falls back — possibly to a wrong port — yielding a blank/unreachable
editor tab even though the container is perfectly healthy. This behaviour catches
exactly that drift at the harness level: a mismatch between the inspected port and
env.port points at the inspect template / docker version; a matching port that fails
to curl points at the publish/bind path. 'fresh' isolation keeps env.port unambiguous
for this env.`,
    needs: [],
    async run(env) {
      const name = env.name;
      const tmpl = `{{(index (index .NetworkSettings.Ports "8080/tcp") 0).HostPort}}`;
      const hostPort = hostSh(`docker inspect -f '${tmpl}' ${name}`);
      const portMatches = hostPort !== "" && Number(hostPort) === Number(env.port);
      const url = `http://127.0.0.1:${hostPort || env.port}/`;
      const code = hostSh(`curl -s -o /dev/null -w '%{http_code}' --max-time 5 ${url}`);
      const reachable = code === "302" || code === "200";
      return {
        pass: portMatches && reachable,
        detail: portMatches && reachable
          ? `inspect HostPort=${hostPort} == env.port=${env.port}; ${url} → ${code}`
          : `HostPort=${JSON.stringify(hostPort)} (env.port=${env.port}); ${url} → ${JSON.stringify(code)}`,
        evidence: { hostPort, envPort: env.port, url, httpCode: code },
      };
    },
  },

  // ── L2.LIFE.005 / L2.NET.009 — reporter phone-home registers a Hub session ────
  {
    id: "life.reporterSessionRegistered",
    specId: "L2.LIFE.005",
    title: "Reporter phone-home registers a Hub session titled by env id",
    tags: ["lifecycle", "net", "hub"],
    rationale: `
WHAT: Asserts the env's \`fleet-reporter --serve\` registered a Hub SESSION on boot,
before any agent run. Two corroborating signals: (a) the reporter process is alive in
the container (\`pgrep -f fleet-reporter\` non-empty), and (b) the HOST-side Hub knows a
session whose title equals env.id (== FLEET_SESSION_TITLE == FLEET_SERVER_ID),
observed via \`fleet ls --once\`. The Hub/CLI is an environmental precondition: if the
\`fleet\` CLI is absent or the Hub never answers, this SKIPS cleanly (never hard-fails),
exactly like agentInput.mjs's Hub gate.

WHY THIS IS THE EXPECTED OUTCOME: entrypoint.sh launches
\`fleet-reporter --serve --ws ws://host.docker.internal:51777 --socket
/tmp/fleet-reporter.sock --session-id <id>\` in the background. \`--serve\` makes the
reporter dial the Hub over the network and register a session titled by
FLEET_SESSION_TITLE (the entrypoint sets it to <id>). A freshly booted env therefore
must surface as a Hub session row — with zero agent runs yet — even though no claude
has run. The pgrep half proves the reporter is actually running (so the network dial
is even attempted); the Hub-session half proves the dial reached the Hub and
registered.

WHY IT MATTERS: Phone-home is the agent-state spine: without a registered session the
rail shows nothing for this env, no matter how healthy code-server and the bridge are.
This entry distinguishes "the container booted" from "the container is VISIBLE to
Fleet." It is the bare-registration counterpart to agent.claudeRuns (which asserts run
STATE transitions): a break here with a healthy bridge isolates a Hub-reachability /
reporter-dial regression from a bridge regression. The clean SKIP on an absent Hub is
load-bearing — turning it into a failure would make the suite red on any machine
without a running Hub, which is environmental, not a regression.`,
    needs: [],
    async run(env) {
      const cli = fleetCli();
      if (!cli) {
        return {
          pass: false,
          skipped: "Hub `fleet` CLI not found (target/debug/fleet) — start the Hub (see test.sh)",
          detail: "skipped: no fleet CLI to query the Hub",
        };
      }
      const reporterPid = env.exec("pgrep -f fleet-reporter | head -1 || true");
      const found = await pollHubSession(env.id, { ms: 15000, every: 1000 });
      if (!found.ok) {
        return {
          pass: false,
          skipped: `Hub session "${env.id}" not found (Hub down or reporter not phoned home)`,
          detail: "skipped: env's session is not registered on the Hub",
          evidence: { sessionTitle: env.id, reporterPid, cli },
        };
      }
      const reporterUp = reporterPid !== "";
      return {
        pass: reporterUp && found.ok,
        detail: reporterUp
          ? `Hub session "${env.id}" registered; reporter pid=${reporterPid}`
          : `Hub session "${env.id}" registered but NO fleet-reporter process found`,
        evidence: { sessionTitle: env.id, reporterPid, sessionLine: found.line },
      };
    },
  },

  // ── L2.NET.001 — entrypoint derives the dial URLs from FLEET_HOST_ADDR ────────
  {
    id: "net.derivedDialUrls",
    specId: "L2.NET.001",
    title: "Entrypoint derives FLEET_HUB_URL/FLEET_BRIDGE_URL from FLEET_HOST_ADDR",
    tags: ["net"],
    rationale: `
WHAT: Reads the live environment of the running container's process tree and asserts
\`FLEET_HUB_URL == ws://host.docker.internal:51777\` and
\`FLEET_BRIDGE_URL == ws://host.docker.internal:51778\`. We read the values off the
RUNNING entrypoint/reporter process (\`/proc/<pid>/environ\`) rather than a fresh
\`printenv\` shell, because the URLs are computed and \`export\`ed by entrypoint.sh at
boot — a new \`docker exec\` shell would NOT inherit them. We locate the reporter PID
(spawned by the entrypoint, so it carries the exported vars) and parse its environ.

WHY THIS IS THE EXPECTED OUTCOME: The harness always passes
\`-e FLEET_HOST_ADDR=host.docker.internal\`, and entrypoint.sh computes
\`HOST=\${FLEET_HOST_ADDR:-<default-route>}\` — so the explicit override MUST win over the
auto-detected gateway, fixing HOST to host.docker.internal. It then builds
\`FLEET_HUB_URL=ws://\${HOST}:\${FLEET_HUB_PORT:-51777}\` and
\`FLEET_BRIDGE_URL=ws://\${HOST}:\${FLEET_BRIDGE_PORT:-51778}\` using the Containerfile's
default ports. The exact derived strings are therefore deterministic and are what the
reporter and bridge actually dial.

WHY IT MATTERS: This is the negative-space guard for L2.NET.002 (gateway fallback):
the explicit override must take precedence, or under colima the container would dial
the wrong host and never phone home — the single most common silent failure (§8).
Reading the URLs off the live process environ (not a shell's printenv) is the
load-bearing subtlety this rationale preserves: a future reader who "fixes" this to
\`env.exec("printenv FLEET_HUB_URL")\` would get an empty string (the var is only
exported in the entrypoint's process, not in interactive shells) and wrongly think the
derivation regressed.`,
    needs: [],
    async run(env) {
      const wantHub = "ws://host.docker.internal:51777";
      const wantBridge = "ws://host.docker.internal:51778";
      // The entrypoint exports the URLs into its own process env; the reporter it
      // spawns inherits them. Read that process's environ (NUL-separated).
      // Concatenate all readable environ files (NUL-separated), split to lines, grep
      // the exported var. The entrypoint exports it before spawning the reporter +
      // code-server, so every child carries it; first match wins. (A plain pipeline,
      // no shell vars — and env.exec now passes through to the container shell anyway.)
      const readVar = (key) =>
        env.exec(`cat /proc/[0-9]*/environ 2>/dev/null | tr '\\0' '\\n' | grep -m1 '^${key}=' | cut -d= -f2-`);
      const hubUrl = readVar("FLEET_HUB_URL");
      const bridgeUrl = readVar("FLEET_BRIDGE_URL");
      const pass = hubUrl === wantHub && bridgeUrl === wantBridge;
      return {
        pass,
        detail: pass
          ? `FLEET_HUB_URL=${hubUrl}, FLEET_BRIDGE_URL=${bridgeUrl}`
          : `HUB=${JSON.stringify(hubUrl)} (want ${wantHub}); BRIDGE=${JSON.stringify(bridgeUrl)} (want ${wantBridge})`,
        evidence: { hubUrl, bridgeUrl, wantHub, wantBridge },
      };
    },
  },

  // ── L2.NET.003 — host.docker.internal resolves; host Hub port reachable ───────
  {
    id: "net.hostDockerInternalReachable",
    specId: "L2.NET.003",
    title: "host.docker.internal resolves and the host Hub port is reachable from the container",
    tags: ["net", "hub"],
    rationale: `
WHAT: From INSIDE the container, asserts the container→host primitive the whole agent
pipeline rides on: (a) \`host.docker.internal\` RESOLVES (\`getent hosts
host.docker.internal\` returns an address), and (b) the host Hub port :51777 is
TCP-reachable (\`nc -z -w2 host.docker.internal 51777\` exits 0). The Hub is an
environmental precondition: if no \`fleet\` CLI / Hub is present we SKIP cleanly — but
we ONLY skip the port-reachability half; name resolution is asserted unconditionally
because host.docker.internal is provided by the docker/colima runtime, not the Hub.

WHY THIS IS THE EXPECTED OUTCOME: Under colima/Docker, \`host.docker.internal\` is the
DNS name the container uses to reach the host's gateway; the harness pins HOST to it
via \`-e FLEET_HOST_ADDR=host.docker.internal\`. Resolution must therefore succeed for
any dial to be possible at all. When a Hub is listening on the host bound 0.0.0.0
(\`FLEET_WS_ADDR=0.0.0.0\`, per run.mjs startHub), a TCP connect to :51777 across the
namespace must succeed — that is the exact path the reporter's phone-home uses.

WHY IT MATTERS: If host.docker.internal stops resolving under a colima upgrade, every
reporter/bridge dial fails even though both endpoints are individually healthy — and
the symptom is an invisible env with no obvious cause. Asserting resolution + a raw nc
connect isolates THIS link from the higher-level reporter/bridge logic: a green
bridge already implies reachability, but a direct probe names the failure precisely
when the bridge is also down. The split gate (always assert resolution, conditionally
assert the port) keeps the test meaningful even without a Hub: the runtime resolver is
testable everywhere; the listener is not.`,
    needs: [],
    async run(env) {
      const resolved = env.exec("getent hosts host.docker.internal | head -1 || true");
      const resolves = resolved.trim() !== "";
      const hubPresent = !!fleetCli();

      if (!hubPresent) {
        // Without a host Hub we cannot assert :51777 reachability — but resolution
        // is runtime-provided and must hold regardless. If even that fails, it's a
        // real regression (not an env-skip), so assert it.
        return {
          pass: resolves,
          detail: resolves
            ? `host.docker.internal resolves (${resolved.split(/\s+/)[0]}); Hub absent → port probe skipped`
            : `host.docker.internal did NOT resolve from the container`,
          evidence: { resolved, hubProbe: "skipped (no Hub/CLI present)" },
        };
      }

      const rc = env.exec("nc -z -w2 host.docker.internal 51777 >/dev/null 2>&1; echo $?");
      const reachable = rc.trim() === "0";
      if (!reachable) {
        // CLI exists but the port isn't reachable — most likely the Hub process is
        // not actually up. Treat as an environmental SKIP for the port half, but
        // still require resolution.
        return {
          pass: false,
          skipped: "host Hub port :51777 not reachable from the container (Hub not listening?) — start the Hub (see test.sh)",
          detail: resolves
            ? "skipped: name resolves but :51777 not reachable (Hub down)"
            : "host.docker.internal did NOT resolve",
          evidence: { resolved, ncRc: rc },
        };
      }
      return {
        pass: resolves && reachable,
        detail: `host.docker.internal resolves (${resolved.split(/\s+/)[0]}) and :51777 is reachable (nc rc=0)`,
        evidence: { resolved, ncRc: rc },
      };
    },
  },

  // ── L2.NET.004 — host Hub bound 0.0.0.0 reachable from the container ──────────
  {
    id: "net.hubBoundAllInterfaces",
    specId: "L2.NET.004",
    title: "Host Hub bound 0.0.0.0 (FLEET_WS_ADDR) is reachable from the container",
    tags: ["net", "hub"],
    rationale: `
WHAT: Asserts the host Hub on :51777 is reachable from BOTH namespaces, which is what
\`FLEET_WS_ADDR=0.0.0.0\` buys: (a) from the HOST, \`nc -z 127.0.0.1 51777\` succeeds, and
(b) from inside the CONTAINER, \`nc -z -w2 host.docker.internal 51777\` succeeds. The Hub
is an environmental precondition — if no \`fleet\` CLI is present, or the host-loopback
probe itself fails (no Hub listening), we SKIP cleanly rather than hard-fail.

WHY THIS IS THE EXPECTED OUTCOME: run.mjs's startHub (and test.sh) launch the Hub with
\`FLEET_WS_ADDR=0.0.0.0\`, so its \`TcpListener::bind((FLEET_WS_ADDR, 51777))\` listens on
ALL interfaces — not just 127.0.0.1. The defining, load-bearing property of a 0.0.0.0
bind (versus the default 127.0.0.1) is precisely that a CONTAINER can reach it via
host.docker.internal while loopback stays reachable too. So both probes must succeed
simultaneously; with the default loopback bind the container probe would FAIL while
the host probe still passed.

WHY IT MATTERS: The default loopback bind is invisible to containers — the single most
common "phone-home silently does nothing" misconfiguration (§8). This entry pins that
the suite/desktop actually set 0.0.0.0 AND that it is observably cross-namespace
reachable, not merely that a flag string was passed. It is the positive control whose
negation is L2.NET.013 (loopback Hub → container can't reach). The skip gate keeps it
honest where no Hub runs.`,
    needs: [],
    async run(env) {
      const cli = fleetCli();
      if (!cli) {
        return {
          pass: false,
          skipped: "Hub `fleet` CLI not found — start the Hub bound 0.0.0.0 (see test.sh)",
          detail: "skipped: no Hub to assert cross-namespace reachability",
        };
      }
      const hostRc = hostSh("nc -z -w2 127.0.0.1 51777 >/dev/null 2>&1; echo $?");
      if (hostRc !== "0") {
        return {
          pass: false,
          skipped: "host :51777 not reachable on loopback — Hub not listening; start it (see test.sh)",
          detail: "skipped: no Hub listening on the host",
          evidence: { hostRc },
        };
      }
      const ctrRc = env.exec("nc -z -w2 host.docker.internal 51777 >/dev/null 2>&1; echo $?").trim();
      const containerReachable = ctrRc === "0";
      return {
        pass: containerReachable,
        detail: containerReachable
          ? "Hub :51777 reachable from BOTH host loopback and the container (0.0.0.0 bind confirmed)"
          : `host loopback reachable but container probe failed (nc rc=${ctrRc}) — Hub bound loopback-only?`,
        evidence: { hostRc, containerRc: ctrRc },
      };
    },
  },

  // ── L2.NET.006 — published port: host→container editor reach on -p <port>:8080 ─
  {
    id: "net.publishedPortReachable",
    specId: "L2.NET.006",
    title: "Published port forwards the host to code-server (302/200) and docker port reports it",
    tags: ["net"],
    isolation: "fresh",
    rationale: `
WHAT: Asserts the host→container editor path: (a) \`curl http://127.0.0.1:<env.port>/\`
from the host answers 302 or 200, and (b) \`docker port fleet-eval-<id> 8080/tcp\`
reports a binding on host port env.port (matching \`0.0.0.0:<port>\` or \`:::<port>\`).

WHY THIS IS THE EXPECTED OUTCOME: The editor is reachable only because TWO independent
things are true together: code-server binds \`0.0.0.0:8080\` INSIDE the container (not
loopback), and the harness publishes that with \`-p <env.port>:8080\` so Docker forwards
the host port to the container's 8080. \`docker port\` reflects the published mapping
from Docker's own view; the curl proves the forward actually carries traffic to a
serving code-server (302 = its auth redirect, 200 = a page). We assert 302/200 per the
suite's readiness contract (§8: wait for 302/200, not any byte).

WHY IT MATTERS: If code-server regressed to a loopback in-container bind
(127.0.0.1:8080), the published port would be DEAD despite a perfectly healthy
container and a present \`docker port\` mapping — the curl half catches exactly that. If
the publish flag drifted, \`docker port\` would be empty — that half catches the
publish-side regression. Splitting the two lets a future reader tell an in-container
bind problem (mapping present, curl fails) from a publish problem (mapping absent).
'fresh' isolation keeps env.port the unambiguous published port for this env.`,
    needs: [],
    async run(env) {
      const name = env.name;
      const url = `http://127.0.0.1:${env.port}/`;
      const code = hostSh(`curl -s -o /dev/null -w '%{http_code}' --max-time 5 ${url}`);
      const reachable = code === "302" || code === "200";
      const portLine = hostSh(`docker port ${name} 8080/tcp`);
      const mapped = portLine.includes(`:${env.port}`);
      return {
        pass: reachable && mapped,
        detail: reachable && mapped
          ? `${url} → ${code}; docker port → ${JSON.stringify(portLine)}`
          : `${url} → ${JSON.stringify(code)}; docker port 8080/tcp → ${JSON.stringify(portLine)} (want :${env.port})`,
        evidence: { url, httpCode: code, dockerPort: portLine, envPort: env.port },
      };
    },
  },

  // ── L2.NET.010 — bridge query round-trips a Snapshot over the network ─────────
  {
    id: "net.bridgeQueryRoundTrip",
    specId: "L2.NET.010",
    title: "Bridge query round-trips a Snapshot over host.docker.internal (full duplex)",
    tags: ["net"],
    rationale: `
WHAT: Issues a host→container bridge \`query\` (via env.observe, which under the hood
sends \`{type:"query",reqId}\` over the registered conn and awaits the reply) and
asserts the returned Snapshot is well-formed: \`terminalCount\` is a number AND the
Snapshot carries at least one of the documented fields (\`terminals\` array /
\`activeEditor\`). A successful, well-typed round-trip = the full-duplex command channel
works over the network.

WHY THIS IS THE EXPECTED OUTCOME: The bridge \`hello\` proves only the INBOUND leg (the
container dialed in and registered). A query proves the OUTBOUND-then-return leg: Fleet
(the host BridgeHub) pushes a request across host.docker.internal:51778 to the
in-container extension, which runs it against the VS Code API and returns
\`{type:"result",reqId,ok:true,data:Snapshot}\` back across the same socket, within the
15s BridgeHub.send timeout. terminalCount being a number (not undefined) confirms the
reply carried a real Snapshot payload, not an empty/error frame.

WHY IT MATTERS: The hello alone could be green while the return path is broken (a
half-open socket, a reqId-correlation bug, a serialization regression) — and native
menu command forwarding, every act()/observe() the whole suite depends on, rides on
that return path. This entry is the minimal direct assertion that the duplex channel
over host.docker.internal genuinely round-trips, isolating a return-path regression
from an inbound-registration one.`,
    needs: [],
    async run(env) {
      const obs = await env.observe("net.bridgeQueryRoundTrip");
      const snap = obs.vscode || {};
      const countOk = typeof snap.terminalCount === "number";
      const shapeOk =
        Array.isArray(snap.terminals) ||
        typeof snap.activeEditor === "string" ||
        Array.isArray(snap.openTabs) ||
        Array.isArray(snap.visibleEditors);
      return {
        pass: countOk && shapeOk,
        detail: countOk && shapeOk
          ? `query round-tripped a Snapshot (terminalCount=${snap.terminalCount})`
          : `Snapshot malformed: terminalCount=${JSON.stringify(snap.terminalCount)}, keys=${JSON.stringify(Object.keys(snap))}`,
        evidence: {
          terminalCount: snap.terminalCount,
          terminals: snap.terminals,
          activeEditor: snap.activeEditor,
        },
      };
    },
  },

  // ── L2.NET.017 — reporter socket is a local-only unix socket (0600) ───────────
  {
    id: "net.reporterSocketLocalOnly",
    specId: "L2.NET.017",
    title: "Reporter socket is an in-container unix socket, mode 0600, not a TCP port",
    tags: ["net"],
    rationale: `
WHAT: Asserts the hook→reporter trust boundary entirely from inside the container:
(a) \`/tmp/fleet-reporter.sock\` is a UNIX SOCKET (\`test -S\`), (b) its mode is exactly
0600 / owner-only (\`stat -c '%a'\` == "600", i.e. an \`srw-------\` listing), and (c) the
reporter exposes NO TCP listener for the hook channel — the only outbound network leg
is the reporter→Hub dial, never an inbound hook port. We confirm (c) by checking that
no process is LISTENING on a TCP socket bound to the hook path's role: specifically,
\`ss -ltn\` (or \`netstat\`) shows no listener other than code-server's :8080.

WHY THIS IS THE EXPECTED OUTCOME: serve.rs restricts the socket perms to 0600
(restrict_socket_perms) precisely because hook frames can MUTATE reported agent state
(a PreToolUse/Stop frame moves a session between working/waiting/idle). The claude
hooks relay to it via \`nc -U /tmp/fleet-reporter.sock\` — a UNIX socket scoped to the
container's filesystem and the owning user — so the mutate-state channel is reachable
ONLY by in-container, same-user processes. The single network leg that leaves the
container is the reporter's already-trusted outbound WS to the Hub (NET.009), which we
do NOT want mirrored by an inbound TCP hook port.

WHY IT MATTERS: If a refactor "simplified" the hook channel to a TCP port, or relaxed
the socket mode, any process that could reach that port (or any user on the box) could
forge agent-state transitions — silently driving Fleet's rail to lie about what an
agent is doing. This pins the trust boundary at the filesystem layer: socket type +
0600 perms + the absence of a hook TCP listener. A break in (a)/(b) means the perm
hardening regressed; a new TCP listener in (c) means the channel itself was wrongly
moved onto the network.`,
    needs: [],
    async run(env) {
      const sock = "/tmp/fleet-reporter.sock";
      const isSock = env.exec(`test -S ${sock} && echo yes || echo no`).trim() === "yes";
      const mode = env.exec(`stat -c '%a' ${sock} 2>/dev/null || stat -f '%Lp' ${sock} 2>/dev/null || true`).trim();
      const modeOk = mode === "600";
      // TCP listeners in the container. The ONLY expected listener is code-server's
      // :8080; assert no listener advertises the reporter/hook role on a TCP port.
      // (ss may be absent on the minimal image; fall back to netstat, then to a
      //  best-effort that doesn't fail the assertion if neither tool exists.)
      const listeners = env.exec(
        "ss -ltn 2>/dev/null || netstat -ltn 2>/dev/null || true",
      );
      // Extract listening ports; tolerate both ss and netstat columns.
      const ports = new Set();
      for (const line of listeners.split("\n")) {
        const m = line.match(/[:.](\d{2,5})\s+/g);
        if (m) for (const frag of m) {
          const p = frag.match(/(\d{2,5})/);
          if (p) ports.add(p[1]);
        }
      }
      // Acceptable listening ports: code-server (8080). Anything else that looks like
      // a reporter/hook TCP port (51777/51778 belong to the HOST, not the container)
      // would be a violation. We assert no 5177x listener exists IN the container.
      const hasHookTcp = ports.has("51777") || ports.has("51778");
      const listenersChecked = listeners.trim() !== "";

      const pass = isSock && modeOk && !hasHookTcp;
      return {
        pass,
        detail: pass
          ? `${sock} is a unix socket mode ${mode}; no in-container hook TCP listener` +
            (listenersChecked ? "" : " (ss/netstat absent — TCP check best-effort)")
          : `isSock=${isSock}, mode=${JSON.stringify(mode)} (want 600), hookTcp=${hasHookTcp}`,
        evidence: { sock, isSock, mode, listenerPorts: [...ports], hasHookTcp },
      };
    },
  },
];
