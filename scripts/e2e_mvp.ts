import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ReviewItem, SpaceRef, SynthesisDoc } from "../frontend/src/lib/types";
import type { SessionReadModel } from "../frontend/src/runtime/types";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = "49411";
const dataDir = mkdtempSync(join(tmpdir(), "fugue-mvp-"));
const baseUrl = `http://127.0.0.1:${port}`;
let authCookie = "";

const apiServer = spawn("./fugue", ["api"], {
  env: {
    ...process.env,
    FUGUE_API_PORT: port,
    FUGUE_DATA_DIR: dataDir
  },
  detached: true,
  stdio: ["ignore", "pipe", "pipe"]
});

let apiServerOutput = "";
apiServer.stdout.on("data", (chunk) => {
  apiServerOutput += chunk.toString();
});
apiServer.stderr.on("data", (chunk) => {
  apiServerOutput += chunk.toString();
});

try {
  await waitForHealth();
  authCookie = await registerUser();

  const initialSpaces = await get<SpaceRef[]>("/api/spaces");
  assert(initialSpaces.length === 0, `fresh databases should have no spaces, got ${initialSpaces.length}`);
  const space = await post<SpaceRef>("/api/spaces", { title: "Session E2E", projection: "belief" });
  const entityProposal = await post<ReviewItem>(`/api/spaces/${space.id}/entities`, {
    role: "issue",
    text: "What evidence would move this claim?"
  });
  const doc = await post<SynthesisDoc>(`/api/review/${entityProposal.id}/accept`, {});
  const entity = doc.nodes.find((node) => node.kind === "entity");
  assert(entity?.kind === "entity", "self-provisioned doc should contain an entity");
  const entityId = entity.id;
  const initialReview = await get<ReviewItem[]>("/api/review");
  assert(initialReview.length === 0, `review lane should be empty after setup, got ${initialReview.length}`);

  const opened = await post<SessionReadModel>("/api/sessions", {
    anchorBlockId: entityId,
    quote: "evidence would move",
    stance: "believe"
  });
  assert(opened.anchor.blockId === entityId, "session should anchor to selected block");
  assert(opened.stance === "believe", "session stance should persist");

  const proposed = await post<ReviewItem>(`/api/sessions/${opened.id}/commit`, {});
  assert(proposed.kind === "session", "session commit should create a session review item");
  assert(proposed.fromSession?.anchorBlockId === entityId, "review item should keep session provenance");

  const acceptedDoc = await post<SynthesisDoc>(`/api/review/${proposed.id}/accept`, {});
  assert(acceptedDoc.version === 3, `accepting review should bump doc to v3, got ${acceptedDoc.version}`);
  assert(
    acceptedDoc.nodes.some((node) => node.kind === "commit" && node.id === "n-commit-3"),
    "accepted doc should contain v3 commit node"
  );

  const stanceCount = sqlite(
    "core.sqlite",
    `select count(*) from stance where target_id = ${literal(entityId)} and axis = 'endorse' and value = 'believe';`
  );
  const eventId = sqlite("operations.sqlite", `select target_id from attention_item where id = ${literal(proposed.id)};`);
  const eventState = sqlite("events.sqlite", `select decision_state from event where id = ${literal(eventId)};`);
  const reviewState = sqlite("operations.sqlite", `select state from attention_item where id = ${literal(proposed.id)};`);
  const docProjection = sqlite("core.sqlite", `select version || '|' || nodes_json from synthesis_doc where space_id = ${literal(space.id)};`);

  assert(stanceCount === "1", `accept should write one stance row, got ${stanceCount}`);
  assert(eventState === "decided", `accept should mark event decided, got ${eventState}`);
  assert(reviewState === "resolved", `accept should resolve review item, got ${reviewState}`);
  assert(docProjection.startsWith("3|"), "doc projection should persist version 3");
  assert(docProjection.includes("n-commit-3"), "doc projection should persist commit node");

  console.log("mvp_e2e=passed");
  console.log(`data_dir=${dataDir}`);
  console.log(`space_count=${initialSpaces.length + 1}`);
  console.log(`initial_doc_version=${doc.version}`);
  console.log(`initial_review_count=${initialReview.length}`);
  console.log(`thread_id=${opened.id}`);
  console.log(`review_id=${proposed.id}`);
  console.log(`accepted_doc_version=${acceptedDoc.version}`);
  console.log(`stance_count=${stanceCount}`);
  console.log(`event_state=${eventState}`);
  console.log(`review_state=${reviewState}`);
} finally {
  await printScenarioCostAtTeardown(baseUrl, "mvp-session-review");
  await stopProcessGroup(apiServer);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") {
    rmSync(dataDir, { recursive: true, force: true });
  }
}

async function registerUser(): Promise<string> {
  const response = await fetch(`${baseUrl}/api/auth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username: "mvp-user", password: "correct horse battery staple" })
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
