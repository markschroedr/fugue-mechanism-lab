import { randomBytes } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = "49429";
const dataDir = mkdtempSync(join(tmpdir(), "fugue-newspaper-"));
const baseUrl = `http://127.0.0.1:${port}`;
const substrateKey = randomBytes(32).toString("hex");
let serverOutput = "";
const apiServer = startApi();

type AgentCredential = { actorId: string; address: string; key: string };
type Space = { id: string; slug: string; name: string };
type Post = { id: string; reference: string };
type WorkerJob = { jobId: string };

try {
  await waitForHealth();
  const alice = await createAgent("alice");
  const bob = await createAgent("bob");
  const space = societyNode<Space>(alice, "space", "create", "--slug", "commons", "--name", "Commons");

  societyNode<Post>(alice, "post", "--space", space.id, "--kind", "note", "--headline", "Private note", "--text", "never publish this", "--direct", bob.address);
  assert(newspaperJobCount(space.id) === 0, "an addressed post must not dirty the public Newspaper");

  const first = societyNode<Post>(alice, "post", "--space", space.id, "--kind", "finding", "--headline", "First public finding", "--text", "Public evidence supports the first position");
  const second = societyNode<Post>(bob, "post", "--space", space.id, "--kind", "question", "--headline", "Public challenge", "--text", "Public evidence also leaves an unresolved challenge");
  assert(newspaperJobCount(space.id) === 1, "multiple public posts should coalesce into one pending Newspaper update");

  const update = await agentPost<WorkerJob>(alice, `/api/board/spaces/${space.id}/newspaper`);
  assert(update.jobId.length > 0, "manual Newspaper update should expose the queued worker job");
  assert(newspaperJobCount(space.id) === 1, "manual update should reuse an already queued Newspaper job");

  await expectGetStatus(`/api/board/spaces/${space.id}/newspaper`, 401);
  const current = societyNode<null>(alice, "newspaper", space.id);
  assert(current === null, "a space should have no Newspaper before its first successful synthesis");

  const payload = JSON.parse(sqlite("operations.sqlite", `select payload_json from worker_job where id = '${escapeSql(update.jobId)}';`)) as { spaceId: string; auto: boolean };
  assert(payload.spaceId === space.id && !payload.auto, "a manual request should upgrade the reused job for immediate execution");
  assert(first.reference !== second.reference, "public sources should keep distinct stable references");

  console.log("newspaper_e2e=passed");
  console.log(`space=${space.id}`);
  console.log(`public_sources=${first.reference},${second.reference}`);
  console.log("private_sources=excluded");
  console.log(`pending_job=${update.jobId}`);
} finally {
  await printScenarioCostAtTeardown(baseUrl, "society-newspaper-core");
  await stopProcessGroup(apiServer);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

function startApi() {
  const env = {
    ...process.env,
    FUGUE_API_PORT: port,
    FUGUE_DATA_DIR: dataDir,
    FUGUE_SUBSTRATE_KEY: substrateKey,
    FUGUE_EXPECT_ZERO_LLM: "1"
  };
  delete env.OPENAI_API_KEY;
  delete env.OPENROUTER_API_KEY;
  const server = spawn("./fugue", ["api"], { env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
  server.stdout.on("data", (chunk) => { serverOutput += chunk.toString(); });
  server.stderr.on("data", (chunk) => { serverOutput += chunk.toString(); });
  return server;
}

async function createAgent(address: string): Promise<AgentCredential> {
  const response = await fetch(`${baseUrl}/api/society/agents`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${substrateKey}` },
    body: JSON.stringify({ address })
  });
  if (!response.ok) throw new Error(`create agent failed: ${response.status} ${await response.text()}`);
  const credential = await response.json() as AgentCredential;
  const configDir = join(dataDir, "node", address);
  mkdirSync(configDir, { recursive: true });
  writeFileSync(join(configDir, "agent.key"), `${credential.key}\n`, { mode: 0o600 });
  writeFileSync(join(configDir, "node.json"), `${JSON.stringify({ apiBaseUrl: baseUrl })}\n`);
  return credential;
}

function societyNode<T>(agent: AgentCredential, ...args: string[]): T {
  const result = spawnSync("python3", ["scripts/society_node.py", ...args], {
    env: {
      ...process.env,
      FUGUE_AGENT_KEY_FILE: join(dataDir, "node", agent.address, "agent.key"),
      FUGUE_API_URL: ""
    },
    encoding: "utf8"
  });
  if (result.status !== 0) throw new Error(`society-node ${args.join(" ")} failed: ${result.stderr || result.stdout}`);
  return JSON.parse(result.stdout) as T;
}

async function agentPost<T>(agent: AgentCredential, path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { authorization: `Bearer ${agent.key}` }
  });
  if (!response.ok) throw new Error(`POST ${path} failed: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

function newspaperJobCount(spaceId: string): number {
  return Number(sqlite("operations.sqlite", `select count(*) from worker_job where kind = 'society_newspaper' and json_extract(payload_json, '$.spaceId') = '${escapeSql(spaceId)}';`));
}

function sqlite(file: string, sql: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`sqlite3 ${file} failed: ${result.stderr}`);
  return result.stdout.trim();
}

function escapeSql(value: string): string {
  return value.replaceAll("'", "''");
}

async function expectGetStatus(path: string, status: number): Promise<void> {
  const response = await fetch(`${baseUrl}${path}`);
  assert(response.status === status, `${path} should return ${status}, got ${response.status}: ${await response.text()}`);
}

async function waitForHealth(): Promise<void> {
  for (let index = 0; index < 900; index += 1) {
    try {
      if ((await fetch(`${baseUrl}/api/health`)).ok) return;
    } catch { /* still starting */ }
    await Bun.sleep(200);
  }
  throw new Error(`API did not become ready:\n${serverOutput}`);
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

async function stopProcessGroup(process: ReturnType<typeof spawn>): Promise<void> {
  if (!process.pid || process.exitCode !== null) return;
  try { globalThis.process.kill(-process.pid, "SIGINT"); } catch { return; }
  const exited = await new Promise<boolean>((resolve) => {
    const timer = setTimeout(() => resolve(false), 2_000);
    process.once("exit", () => { clearTimeout(timer); resolve(true); });
  });
  if (!exited) {
    try { globalThis.process.kill(-process.pid, "SIGKILL"); } catch { /* exited */ }
  }
}
