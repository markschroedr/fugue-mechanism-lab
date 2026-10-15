import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ReviewItem, SpaceRef, SynthesisDoc } from "../frontend/src/lib/types";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = "49413";
const dataDir = mkdtempSync(join(tmpdir(), "fugue-entities-"));
const baseUrl = `http://127.0.0.1:${port}`;
let authCookie = "";

let apiServerOutput = "";
let apiServer = startApi();

try {
  await waitForHealth();
  authCookie = await registerUser();

  const space = await post<SpaceRef>("/api/spaces", {
    title: "Slice 4 live space",
    projection: "belief"
  });
  assert(space.title === "Slice 4 live space", "created space should return its title");
  assert(space.projection === "belief", "created space should keep requested projection");

  const spaces = await get<SpaceRef[]>("/api/spaces");
  assert(spaces.some((candidate) => candidate.id === space.id), "created space should appear in /api/spaces");

  const emptyDoc = await get<SynthesisDoc>(`/api/spaces/${space.id}/doc`);
  assert(emptyDoc.version === 1, `empty doc should start at v1, got ${emptyDoc.version}`);
  assert(emptyDoc.nodes.length === 0, "new space doc should start empty");

  const proposed = await post<ReviewItem>(`/api/spaces/${space.id}/entities`, {
    role: "issue",
    text: "Does this new issue survive the review lane?"
  });
  assert(proposed.kind === "entity", `new entity proposal should use entity review kind, got ${proposed.kind}`);
  assert(proposed.summary.includes("issue"), "review item should summarize the proposed role");
  assert(proposed.detail?.includes("survive the review lane"), "review item should show the proposed text");

  const review = await get<ReviewItem[]>("/api/review");
  assert(review.some((item) => item.id === proposed.id), "entity proposal should appear in review lane");

  const eventId = sqlite("operations.sqlite", `select target_id from attention_item where id = ${literal(proposed.id)};`);
  const proposedEventState = sqlite("events.sqlite", `select decision_state from event where id = ${literal(eventId)};`);
  const proposedEventPayload = sqlite("events.sqlite", `select payload_json from event where id = ${literal(eventId)};`);
  assert(proposedEventState === "proposed", `new entity event should be proposed, got ${proposedEventState}`);
  assert(!proposedEventPayload.includes("survive the review lane"), "event payload should not duplicate entity text");

  sqlite("operations.sqlite", `create trigger fail_synthesis_insert before insert on worker_job
    when new.kind = 'synthesize' begin select raise(abort, 'injected queue failure'); end;`);
  const failedAccept = await fetch(`${baseUrl}/api/review/${proposed.id}/accept`, {
    method: "POST", headers: { cookie: authCookie }
  });
  assert(failedAccept.status === 500, "queue failure must fail acceptance");
  assert(sqlite("operations.sqlite", `select state from attention_item where id = ${literal(proposed.id)}`) === "pending",
    "queue failure must leave review pending, not partly accepted");
  assert(sqlite("core.sqlite", `select count(*) from entity where space_id = ${literal(space.id)}`) === "0",
    "queue failure must roll back the entity and its projection");
  assert(sqlite("events.sqlite", `select decision_state from event where id = ${literal(eventId)}`) === "proposed",
    "queue failure must roll back the decision");
  sqlite("operations.sqlite", "drop trigger fail_synthesis_insert;");

  const acceptedDoc = await post<SynthesisDoc>(`/api/review/${proposed.id}/accept`, {});
  assert(acceptedDoc.version === 2, `accepting entity should bump doc to v2, got ${acceptedDoc.version}`);
  const entity = acceptedDoc.nodes.find((node) => node.kind === "entity");
  assert(entity?.kind === "entity", "accepted doc should contain the new entity node");
  assert(entity.role === "issue", `entity node should expose role issue, got ${entity.role}`);
  assert(entity.state === "open", `new issue should start open, got ${entity.state}`);
  assert(entity.text === "Does this new issue survive the review lane?", "entity node should contain accepted text");

  const entityId = entity.id;
  const entityRow = sqlite("core.sqlite", `select role || '|' || state || '|' || content from entity where id = ${literal(entityId)};`);
  const decidedEventState = sqlite("events.sqlite", `select decision_state from event where id = ${literal(eventId)};`);
  const reviewState = sqlite("operations.sqlite", `select state from attention_item where id = ${literal(proposed.id)};`);
  assert(entityRow === "issue|open|Does this new issue survive the review lane?", `accepted entity row mismatch: ${entityRow}`);
  assert(decidedEventState === "decided", `accept should mark event decided, got ${decidedEventState}`);
  assert(reviewState === "resolved", `accept should resolve review item, got ${reviewState}`);

  sqlite("operations.sqlite", `update worker_job set updated_at = '2099-01-01T00:00:00Z'
    where kind = 'synthesize' and state = 'queued' and target_id = ${literal(space.id)};
    create trigger fail_synthesis_update before update on worker_job
    when new.kind = 'synthesize' begin select raise(abort, 'injected debounce failure'); end;`);
  for (const [method, path, body] of [
    ["PATCH", `/api/entities/${entityId}`, { text: "Must roll back" }],
    ["POST", `/api/entities/${entityId}/state`, { state: "resolved" }]
  ] as const) {
    const failedMutation = await fetch(`${baseUrl}${path}`, {
      method, headers: { "content-type": "application/json", cookie: authCookie }, body: JSON.stringify(body)
    });
    assert(failedMutation.status === 500, "debounce failure must fail the mutation");
    assert(sqlite("core.sqlite", `select role || '|' || state || '|' || content from entity where id = ${literal(entityId)}`) === entityRow,
      "debounce failure must roll back entity text and state");
    assert(sqlite("core.sqlite", `select version from synthesis_doc where space_id = ${literal(space.id)}`) === "2",
      "debounce failure must roll back the compiled projection");
  }
  sqlite("operations.sqlite", "drop trigger fail_synthesis_update;");
  console.log("synthesis_queue_atomicity=accept_insert,edit_update,state_update_rolled_back");

  const editedDoc = await patch<SynthesisDoc>(`/api/entities/${entityId}`, {
    text: "Does the revised issue update the compiled projection?"
  });
  assert(editedDoc.version === 3, `editing entity should bump doc to v3, got ${editedDoc.version}`);
  assert(
    editedDoc.nodes.some((node) => node.kind === "entity" && node.id === entityId && node.text === "Does the revised issue update the compiled projection?"),
    "edited doc should contain revised entity text"
  );

  const editEvent = sqlite("events.sqlite", `select decision_state || '|' || kind from event where target_id = ${literal(entityId)} and kind = 'entity_text_updated';`);
  const editEventPayload = sqlite("events.sqlite", `select payload_json from event where target_id = ${literal(entityId)} and kind = 'entity_text_updated';`);
  assert(editEvent === "decided|entity_text_updated", `edit should emit decided event, got ${editEvent}`);
  assert(!editEventPayload.includes("revised issue"), "edit event payload should not duplicate entity text");

  const resolvedDoc = await post<SynthesisDoc>(`/api/entities/${entityId}/state`, {
    state: "resolved"
  });
  assert(resolvedDoc.version === 4, `resolving entity should bump doc to v4, got ${resolvedDoc.version}`);
  assert(
    resolvedDoc.nodes.some((node) => node.kind === "entity" && node.id === entityId && node.state === "resolved"),
    "resolved doc should contain resolved entity state"
  );

  const stateEvent = sqlite("events.sqlite", `select decision_state || '|' || kind from event where target_id = ${literal(entityId)} and kind = 'entity_state_changed';`);
  const finalEntityRow = sqlite("core.sqlite", `select state || '|' || content from entity where id = ${literal(entityId)};`);
  const projectionRow = sqlite("core.sqlite", `select version || '|' || nodes_json from synthesis_doc where space_id = ${literal(space.id)};`);
  assert(stateEvent === "decided|entity_state_changed", `resolve should emit decided event, got ${stateEvent}`);
  assert(finalEntityRow === "resolved|Does the revised issue update the compiled projection?", `resolved entity row mismatch: ${finalEntityRow}`);
  assert(projectionRow.startsWith("4|"), "doc projection should persist version 4");
  assert(projectionRow.includes("Does the revised issue update the compiled projection?"), "doc projection should persist revised text");

  await stopProcessGroup(apiServer);
  apiServer = startApi();
  await waitForHealth();
  const reloadedDoc = await get<SynthesisDoc>(`/api/spaces/${space.id}/doc`);
  assert(reloadedDoc.version === 4, `reloaded doc should keep v4, got ${reloadedDoc.version}`);
  assert(
    reloadedDoc.nodes.some((node) => node.kind === "entity" && node.id === entityId && node.state === "resolved"),
    "reloaded doc should keep resolved entity"
  );

  console.log("entity_interactions_e2e=passed");
  console.log(`data_dir=${dataDir}`);
  console.log(`space_id=${space.id}`);
  console.log(`entity_id=${entityId}`);
  console.log(`review_id=${proposed.id}`);
  console.log(`proposal_event_state=${proposedEventState}`);
  console.log(`accepted_doc_version=${acceptedDoc.version}`);
  console.log(`edited_doc_version=${editedDoc.version}`);
  console.log(`resolved_doc_version=${resolvedDoc.version}`);
  console.log(`reloaded_doc_version=${reloadedDoc.version}`);
  console.log(`edit_event=${editEvent}`);
  console.log(`state_event=${stateEvent}`);
} finally {
  await printScenarioCostAtTeardown(baseUrl, "entity-interactions");
  await stopProcessGroup(apiServer);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") {
    rmSync(dataDir, { recursive: true, force: true });
  }
}

function startApi() {
  apiServerOutput = "";
  const server = spawn("./fugue", ["api"], {
    env: {
      ...process.env,
      FUGUE_API_PORT: port,
      FUGUE_DATA_DIR: dataDir
    },
    detached: true,
    stdio: ["ignore", "pipe", "pipe"]
  });

  server.stdout.on("data", (chunk) => {
    apiServerOutput += chunk.toString();
  });
  server.stderr.on("data", (chunk) => {
    apiServerOutput += chunk.toString();
  });

  return server;
}

async function registerUser(): Promise<string> {
  const response = await fetch(`${baseUrl}/api/auth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username: "e2e-user", password: "correct horse battery staple" })
  });
  if (!response.ok) throw new Error(`registration failed: ${response.status} ${await response.text()}`);
  const cookie = response.headers.get("set-cookie")?.split(";", 1)[0];
  if (!cookie) throw new Error("registration did not set a session cookie");
  return cookie;
}

async function waitForHealth() {
  const startedAt = Date.now();
  while (Date.now() - startedAt < 20_000) {
    try {
      const response = await fetch(`${baseUrl}/api/health`);
      if (response.ok) return;
    } catch {
      // Server is still starting.
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }

  throw new Error(`API did not become ready. Server output:\n${apiServerOutput}`);
}

async function get<T>(path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, { headers: { cookie: authCookie } });
  if (!response.ok) {
    throw new Error(`${path} failed with ${response.status}: ${await response.text()}`);
  }
  return (await response.json()) as T;
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: authCookie },
    body: JSON.stringify(body)
  });
  if (!response.ok) {
    throw new Error(`${path} failed with ${response.status}: ${await response.text()}`);
  }
  return (await response.json()) as T;
}

async function patch<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "PATCH",
    headers: { "content-type": "application/json", cookie: authCookie },
    body: JSON.stringify(body)
  });
  if (!response.ok) {
    throw new Error(`${path} failed with ${response.status}: ${await response.text()}`);
  }
  return (await response.json()) as T;
}

function sqlite(file: string, sql: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], {
    encoding: "utf8"
  });
  if (result.status !== 0) {
    throw new Error(`sqlite3 ${file} failed: ${result.stderr}`);
  }
  return result.stdout.trim();
}

function literal(value: string): string {
  return `'${value.replaceAll("'", "''")}'`;
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

async function stopProcessGroup(process: ReturnType<typeof spawn>) {
  if (!process.pid || process.exitCode !== null) return;

  try {
    globalThis.process.kill(-process.pid, "SIGINT");
  } catch {
    return;
  }

  const exited = await new Promise<boolean>((resolve) => {
    const timer = setTimeout(() => resolve(false), 2_000);
    process.once("exit", () => {
      clearTimeout(timer);
      resolve(true);
    });
  });

  if (!exited) {
    try {
      globalThis.process.kill(-process.pid, "SIGKILL");
    } catch {
      // Process group already exited.
    }
  }
}
