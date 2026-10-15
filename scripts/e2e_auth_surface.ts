import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ReviewItem, SpaceRef, SynthesisDoc } from "../frontend/src/lib/types";
import type { AuthActor } from "../frontend/src/runtime/types";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = "49419";
const dataDir = mkdtempSync(join(tmpdir(), "fugue-auth-surface-"));
const baseUrl = `http://127.0.0.1:${port}`;
const server = spawn("./fugue", ["api"], {
  env: { ...process.env, FUGUE_API_PORT: port, FUGUE_DATA_DIR: dataDir },
  detached: true,
  stdio: ["ignore", "pipe", "pipe"]
});
let serverOutput = "";
server.stdout.on("data", (chunk) => (serverOutput += chunk.toString()));
server.stderr.on("data", (chunk) => (serverOutput += chunk.toString()));

try {
  await waitForHealth();
  assert((await json<SpaceRef[]>("/api/spaces")).length === 0, "fresh databases should expose an empty space list");

  const registered = await auth("register", "surface-user", "a durable test password");
  const actor = registered.body;
  const spaceResponse = await fetch(`${baseUrl}/api/spaces`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: registered.cookie },
    body: JSON.stringify({ title: "Auth surface proof", projection: "belief" })
  });
  assert(spaceResponse.ok, `authenticated space creation failed: ${spaceResponse.status}`);
  const space = (await spaceResponse.json()) as SpaceRef;

  await fetch(`${baseUrl}/api/auth/logout`, { method: "POST", headers: { cookie: registered.cookie } });
  const anonymousReport = await fetch(`${baseUrl}/api/spaces/${space.id}/report`);
  const anonymousHistory = await fetch(`${baseUrl}/api/spaces/${space.id}/report/history`);
  const anonymousHolders = await fetch(`${baseUrl}/api/spaces/${space.id}/doc`);
  assert(anonymousReport.ok && (await anonymousReport.json()) === null, "anonymous report read should return 200/null before synthesis");
  assert(anonymousHistory.ok && ((await anonymousHistory.json()) as unknown[]).length === 0, "anonymous history read should return 200/empty");
  assert(anonymousHolders.ok && ((await anonymousHolders.json()) as SynthesisDoc).nodes.length === 0, "anonymous holder/doc read should return 200");

  const anonymousComposer = await fetch(`${baseUrl}/api/spaces/${space.id}/input`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ kind: "text", content: "anonymous mutation" })
  });
  const anonymousCreate = await fetch(`${baseUrl}/api/spaces`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ title: "must fail", projection: "belief" })
  });
  assert(anonymousComposer.status === 401 && anonymousCreate.status === 401, "anonymous composer and create writes must return 401");

  const loggedIn = await auth("login", "surface-user", "a durable test password");
  assert(loggedIn.body.actorId === actor.actorId, "login should restore the registered actor");
  const proposalResponse = await fetch(`${baseUrl}/api/spaces/${space.id}/entities`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: loggedIn.cookie },
    body: JSON.stringify({ role: "issue", text: "Packaging capacity may bind before capex reroutes around it." })
  });
  if (!proposalResponse.ok) throw new Error(`authenticated entity proposal failed: ${proposalResponse.status} ${await proposalResponse.text()}`);
  const review = (await proposalResponse.json()) as ReviewItem;

  const anonymousAccept = await fetch(`${baseUrl}/api/review/${review.id}/accept`, { method: "POST" });
  assert(anonymousAccept.status === 401, `anonymous review acceptance should return 401, got ${anonymousAccept.status}`);
  const intruder = await auth("register", "other-user", "another durable password");
  const accepted = await fetch(`${baseUrl}/api/review/${review.id}/accept`, {
    method: "POST",
    headers: { cookie: intruder.cookie }
  });
  if (!accepted.ok) throw new Error(`authenticated review acceptance failed: ${accepted.status} ${await accepted.text()}`);
  const acceptedDoc = (await accepted.json()) as SynthesisDoc;
  const entity = acceptedDoc.nodes.find((node) => node.kind === "entity");
  assert(entity?.kind === "entity", "accepted input should compile an entity");
  const opened = await fetch(`${baseUrl}/api/sessions`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: loggedIn.cookie },
    body: JSON.stringify({ anchorBlockId: entity.id, stance: "believe" })
  });
  assert(opened.ok, "owner should be able to open a private session");
  const threadId = ((await opened.json()) as { id: string }).id;
  const anonymousSessionRead = await fetch(`${baseUrl}/api/sessions/${threadId}`);
  const anonymousReviewRead = await fetch(`${baseUrl}/api/review`);
  const intruderSessionRead = await fetch(`${baseUrl}/api/sessions/${threadId}`, { headers: { cookie: intruder.cookie } });
  assert(anonymousSessionRead.status === 401 && anonymousReviewRead.status === 401, "private session and review reads must require authentication");
  assert(intruderSessionRead.status === 404, "a different actor must not read another actor's session");

  const proposalEventId = sqlite("operations.sqlite", `select target_id from attention_item where id = ${literal(review.id)};`);
  const proposedBy = sqlite("events.sqlite", `select actor_id from event where id = ${literal(proposalEventId)};`);
  const confirmedBy = sqlite("core.sqlite", `select confirmed_by from entity where space_id = ${literal(space.id)} limit 1;`);
  const storedHash = sqlite("operations.sqlite", `select password_hash from credential where actor_id = ${literal(actor.actorId)};`);
  const createdBy = sqlite("core.sqlite", `select created_by from entity where id = ${literal(entity.id)};`);
  assert(proposedBy === actor.actorId && createdBy === actor.actorId && confirmedBy === intruder.body.actorId,
    "review must preserve the contributor as creator and record the separate reviewer as confirmer");
  const committed = await fetch(`${baseUrl}/api/sessions/${threadId}/commit`, {
    method: "POST", headers: { cookie: loggedIn.cookie }
  });
  assert(committed.ok, "contributor should be able to propose their session");
  const sessionReview = await committed.json() as ReviewItem;
  const sessionAccepted = await fetch(`${baseUrl}/api/review/${sessionReview.id}/accept`, {
    method: "POST", headers: { cookie: intruder.cookie }
  });
  assert(sessionAccepted.ok, "a reviewer should be able to accept the contributor's session");
  const sessionDoc = await sessionAccepted.json() as SynthesisDoc;
  assert(sqlite("core.sqlite", `select actor_id from stance where target_id = ${literal(entity.id)};`) === actor.actorId,
    "review acceptance must not assign the contributor's belief to the reviewer");
  assert(sessionDoc.nodes.some((node) => node.kind === "commit" && node.author === actor.username),
    "shared commit markers must name the contributor rather than the reader");
  console.log("review_attribution=contributor_creation_and_stance,reviewer_confirmation,shared_named_commit");
  assert(storedHash.startsWith("$argon2"), "password must be stored as an argon2 hash");

  console.log("auth_surface_e2e=passed");
  console.log(`actor_id=${actor.actorId}`);
  console.log(`space_id=${space.id}`);
  console.log(`anonymous_reads=${anonymousReport.status},${anonymousHistory.status},${anonymousHolders.status}`);
  console.log(`anonymous_writes=${anonymousComposer.status},${anonymousCreate.status},${anonymousAccept.status}`);
  console.log(`private_read_gates=${anonymousSessionRead.status},${anonymousReviewRead.status},${intruderSessionRead.status}`);
  console.log(`entity_proposed_by=${proposedBy}`);
  console.log(`entity_confirmed_by=${confirmedBy}`);
  console.log(`password_hash_prefix=${storedHash.slice(0, 9)}`);
} finally {
  await printScenarioCostAtTeardown(baseUrl, "auth-surface");
  await stopProcessGroup(server);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

async function auth(action: "register" | "login", username: string, password: string): Promise<{ body: AuthActor; cookie: string }> {
  const response = await fetch(`${baseUrl}/api/auth/${action}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username, password })
  });
  if (!response.ok) throw new Error(`${action} failed: ${response.status} ${await response.text()}`);
  const cookie = response.headers.get("set-cookie")?.split(";", 1)[0];
  assert(cookie, `${action} should set a session cookie`);
  return { body: (await response.json()) as AuthActor, cookie };
}

async function json<T>(path: string, cookie?: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, { headers: cookie ? { cookie } : {} });
  if (!response.ok) throw new Error(`${path} failed: ${response.status} ${await response.text()}`);
  return (await response.json()) as T;
}

async function waitForHealth() {
  const started = Date.now();
  while (Date.now() - started < 20_000) {
    try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch { /* starting */ }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`API did not start. ${serverOutput}`);
}

function sqlite(file: string, sql: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`sqlite3 ${file} failed: ${result.stderr}`);
  return result.stdout.trim();
}
function literal(value: string) { return `'${value.replaceAll("'", "''")}'`; }
function assert(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
async function stopProcessGroup(process: ReturnType<typeof spawn>) {
  if (!process.pid || process.exitCode !== null) return;
  try { globalThis.process.kill(-process.pid, "SIGINT"); } catch { return; }
  await new Promise((resolve) => setTimeout(resolve, 400));
  if (process.exitCode === null) try { globalThis.process.kill(-process.pid, "SIGKILL"); } catch { /* exited */ }
}
