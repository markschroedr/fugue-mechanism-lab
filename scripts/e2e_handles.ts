import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { DocNode, FeedEntry, RefreshSchedule, ReviewItem, SpaceRef, SynthesisDoc } from "../frontend/src/lib/types";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

type FalsifierGroup = {
  positionId: string;
  positionText: string;
  falsifiers: Array<{
    contractId: string;
    kind: "falsifier" | "conditionalStance";
    actorId: string;
    actorName: string;
    terms: string;
    conditionEntityId: string;
    conditionText: string;
    evidence: { artifactId: string };
  }>;
};

type Session = { id: string };

const port = String(49531 + Math.floor(Math.random() * 100));
const dataDir = mkdtempSync(join(tmpdir(), "fugue-handles-"));
const baseUrl = `http://127.0.0.1:${port}`;
let authCookie = "";
let serverOutput = "";
let server = startApi();

try {
  await waitForHealth();
  authCookie = await registerUser();
  const space = await post<SpaceRef>("/api/spaces", { title: "Coordination handles", projection: "belief" });
  const initialDoc = await get<SynthesisDoc>(`/api/spaces/${space.id}/doc`);
  assert(isIsoTimestamp(initialDoc.compiledAt), `compiledAt must be a real ISO timestamp, got ${initialDoc.compiledAt}`);

  const position = await proposeAndAccept(space.id, "issue", "Deployment restraint remains warranted");
  assert(position.role === "issue", "position must be an issue");

  const reviewedCondition = await post<ReviewItem>(`/api/spaces/${space.id}/entities`, {
    role: "condition",
    text: "A monitored deployment causes no severe incident for twelve months"
  });
  assert(reviewedCondition.kind === "entity" && reviewedCondition.summary.includes("condition"), "condition role must use the review lane");
  const conditionDoc = await post<SynthesisDoc>(`/api/review/${reviewedCondition.id}/accept`, {});
  const condition = conditionDoc.nodes.find((node) => node.kind === "entity" && node.role === "condition");
  assert(condition?.kind === "entity" && condition.state === "open", "accepted condition must persist as an open condition entity");

  const session = await post<Session>("/api/sessions", { anchorBlockId: position.id, stance: "believe" });
  const stanceReview = await post<ReviewItem>(`/api/sessions/${session.id}/commit`, {});
  await post<SynthesisDoc>(`/api/review/${stanceReview.id}/accept`, {});

  const terms = "I would reconsider if a monitored deployment causes no severe incident for twelve months.";
  const declaration = await post<ReviewItem>(`/api/spaces/${space.id}/conditions`, {
    targetEntityId: position.id,
    terms
  });
  assert(declaration.kind === "entity", "native condition declaration must enter the entity review lane");
  const declarationEventId = sqlite("operations.sqlite", `select target_id from attention_item where id = ${literal(declaration.id)};`);
  const declaredEvidenceId = sqlite("events.sqlite", `select json_extract(payload_json, '$.evidenceArtifactId') from event where id = ${literal(declarationEventId)};`);
  const evidenceAtDeclaration = sqlite("core.sqlite", `select content || '|' || json_extract(metadata_json, '$.submittedBy') from artifact where id = ${literal(declaredEvidenceId)};`);
  assert(evidenceAtDeclaration.startsWith(`${terms}|actor_`), "declaration must immediately create attributed exact evidence before review");
  const pendingFeed = await get<FeedEntry[]>(`/api/spaces/${space.id}/feed`);
  assert(!pendingFeed.some((entry) => entry.eventId === declarationEventId), "pending condition must not appear in the accepted change feed");
  const declarerActorId = evidenceAtDeclaration.slice(terms.length + 1);
  const declarerCookie = authCookie;
  authCookie = await registerUser("review-operator");
  const declaredDoc = await post<SynthesisDoc>(`/api/review/${declaration.id}/accept`, {});
  authCookie = declarerCookie;
  const declaredCondition = declaredDoc.nodes.find(
    (node) => node.kind === "entity" && node.role === "condition" && node.text === terms
  );
  assert(declaredCondition?.kind === "entity", "acceptance must create the declared condition entity");

  const contract = sqlite(
    "core.sqlite",
    `select kind || '|' || status || '|' || actor_id || '|' || target_entity_id || '|' || condition_entity_id || '|' || evidence_artifact_id || '|' || terms from contract where kind = 'conditional_stance';`
  );
  assert(contract.includes(`conditional_stance|open|${declarerActorId}|`) && contract.endsWith(`|${terms}`), `reviewer must not change the conditional contract's declarer: ${contract}`);
  const evidenceId = contract.split("|")[5] ?? "";
  const evidence = sqlite("core.sqlite", `select content || '|' || json_extract(metadata_json, '$.submittedBy') from artifact where id = ${literal(evidenceId)};`);
  assert(evidenceId === declaredEvidenceId, "accepted contract must retain the artifact created at declaration");
  assert(evidence.startsWith(`${terms}|actor_`), `declaration must create attributed exact evidence, got ${evidence}`);

  const feed = await get<FeedEntry[]>(`/api/spaces/${space.id}/feed`);
  const conditionFeedEntry = feed.find((entry) => entry.eventId === declarationEventId);
  assert(conditionFeedEntry?.kind === "cause-accepted", "accepted native condition must appear in the change feed");
  assert(conditionFeedEntry.label === "Accepted a condition on a stance", "condition feed entry must be legible");
  assert(conditionFeedEntry.cause?.source?.artifactId === evidenceId, "condition feed entry must retain exact evidence");

  const groups = await get<FalsifierGroup[]>(`/api/spaces/${space.id}/falsifiers`);
  const group = groups.find((candidate) => candidate.positionId === position.id);
  assert(group?.falsifiers.length === 1, "falsifier read model must group the condition under its target position");
  assert(group.falsifiers[0]?.terms === terms && group.falsifiers[0]?.evidence.artifactId === evidenceId, "falsifier read must retain exact terms and evidence");

  const holderDoc = await get<SynthesisDoc>(`/api/spaces/${space.id}/doc`);
  const holderPosition = holderDoc.nodes.find((node) => node.kind === "entity" && node.id === position.id);
  assert(holderPosition?.kind === "entity" && holderPosition.holders[0]?.conditions[0]?.terms === terms, "holder-panel model must expose conditions deterministically");

  const duplicate = await request(`/api/spaces/${space.id}/conditions`, {
    method: "POST",
    headers: jsonHeaders(),
    body: JSON.stringify({ targetEntityId: position.id, terms })
  });
  assert(duplicate.status === 400, `duplicate declaration must be rejected, got ${duplicate.status}`);
  assert(Number(sqlite("core.sqlite", "select count(*) from contract where kind = 'conditional_stance';")) === 1, "contract uniqueness must hold in storage");

  const schedule = await put<RefreshSchedule>(`/api/spaces/${space.id}/refresh-schedule`, { cadenceSeconds: 604800, enabled: true });
  assert(schedule.cadenceSeconds === 604800, "schedule write must persist weekly cadence");
  await stopProcessGroup(server);
  server = startApi();
  await waitForHealth();
  const rehydrated = await get<RefreshSchedule | null>(`/api/spaces/${space.id}/refresh-schedule`);
  assert(rehydrated?.cadenceSeconds === 604800 && rehydrated.enabled, "schedule read must rehydrate persisted cadence after restart");

  console.log("coordination_handles_e2e=passed");
  console.log(`space_id=${space.id}`);
  console.log(`position_id=${position.id}`);
  console.log(`reviewed_condition_id=${condition.id}`);
  console.log(`declared_condition_id=${declaredCondition.id}`);
  console.log(`evidence_artifact_id=${evidenceId}`);
  console.log(`contract_row=${contract}`);
  console.log(`condition_feed_entry=${JSON.stringify(conditionFeedEntry)}`);
  console.log(`falsifier_groups=${JSON.stringify(groups)}`);
  console.log(`rehydrated_schedule=${JSON.stringify(rehydrated)}`);
  console.log(`compiled_at=${initialDoc.compiledAt}`);
} finally {
  await printScenarioCostAtTeardown(baseUrl, "coordination-handles");
  await stopProcessGroup(server);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

async function proposeAndAccept(spaceId: string, role: string, text: string): Promise<Extract<DocNode, { kind: "entity" }>> {
  const review = await post<ReviewItem>(`/api/spaces/${spaceId}/entities`, { role, text });
  const doc = await post<SynthesisDoc>(`/api/review/${review.id}/accept`, {});
  const entity = doc.nodes.find((node) => node.kind === "entity" && node.text === text);
  if (!entity || entity.kind !== "entity") throw new Error(`accepted ${role} missing from document`);
  return entity;
}

function startApi() {
  serverOutput = "";
  const child = spawn("./fugue", ["api"], {
    env: { ...process.env, FUGUE_API_PORT: port, FUGUE_DATA_DIR: dataDir },
    detached: true,
    stdio: ["ignore", "pipe", "pipe"]
  });
  child.stdout.on("data", (chunk) => (serverOutput += chunk.toString()));
  child.stderr.on("data", (chunk) => (serverOutput += chunk.toString()));
  return child;
}

async function registerUser(username = "handles-user") {
  const response = await fetch(`${baseUrl}/api/auth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username, password: "correct horse battery staple" })
  });
  if (!response.ok) throw new Error(`registration failed: ${response.status} ${await response.text()}`);
  const cookie = response.headers.get("set-cookie")?.split(";", 1)[0];
  if (!cookie) throw new Error("registration did not set a cookie");
  return cookie;
}

async function waitForHealth() {
  const started = Date.now();
  while (Date.now() - started < 30_000) {
    try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch { /* starting */ }
    await Bun.sleep(200);
  }
  throw new Error(`API did not start:\n${serverOutput}`);
}

function jsonHeaders() { return { "content-type": "application/json", cookie: authCookie }; }
async function request(path: string, init?: RequestInit) { return fetch(`${baseUrl}${path}`, { ...init, headers: { cookie: authCookie, ...(init?.headers ?? {}) } }); }
async function get<T>(path: string): Promise<T> { const response = await request(path); if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`); return response.json() as Promise<T>; }
async function post<T>(path: string, body: unknown): Promise<T> { const response = await request(path, { method: "POST", headers: jsonHeaders(), body: JSON.stringify(body) }); if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`); return response.json() as Promise<T>; }
async function put<T>(path: string, body: unknown): Promise<T> { const response = await request(path, { method: "PUT", headers: jsonHeaders(), body: JSON.stringify(body) }); if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`); return response.json() as Promise<T>; }
function sqlite(file: string, sql: string) { const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" }); if (result.status !== 0) throw new Error(`sqlite3 ${file}: ${result.stderr}`); return result.stdout.trim(); }
function literal(value: string) { return `'${value.replaceAll("'", "''")}'`; }
function isIsoTimestamp(value: string) { return /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z$/.test(value) && !Number.isNaN(Date.parse(value)); }
function assert(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
async function stopProcessGroup(process: ReturnType<typeof spawn>) {
  if (!process.pid || process.exitCode !== null) return;
  try { globalThis.process.kill(-process.pid, "SIGINT"); } catch { return; }
  const exited = await new Promise<boolean>((resolve) => { const timer = setTimeout(() => resolve(false), 2_000); process.once("exit", () => { clearTimeout(timer); resolve(true); }); });
  if (!exited) try { globalThis.process.kill(-process.pid, "SIGKILL"); } catch { /* exited */ }
}
