import { randomBytes } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = String(49600 + Math.floor(Math.random() * 200));
const dataDir = mkdtempSync(join(tmpdir(), "fugue-newspaper-live-"));
const baseUrl = `http://127.0.0.1:${port}`;
const substrateKey = randomBytes(32).toString("hex");
const privateSentinel = "PRIVATE_BUDGET_CEILING_7319";
const env = liveEnvironment();
let serverOutput = "";
const apiServer = spawn("./fugue", ["api"], {
  env,
  detached: true,
  stdio: ["ignore", "pipe", "pipe"]
});
apiServer.stdout.on("data", (chunk) => { serverOutput += chunk.toString(); });
apiServer.stderr.on("data", (chunk) => { serverOutput += chunk.toString(); });

type AgentCredential = { actorId: string; address: string; key: string };
type Space = { id: string; slug: string; name: string };
type Post = { id: string; reference: string };
type WorkerJob = { jobId: string };
type NewspaperEntry = { text: string; postReferences: string[] };
type Newspaper = {
  artifactId: string;
  version: number;
  content: string;
  throughPostReference: string;
  sections: {
    sourceRecord: NewspaperEntry[];
    synthesisInterpretation: NewspaperEntry[];
    openQuestions: NewspaperEntry[];
  };
};
type CostSummary = {
  overall: { llmCalls: number; inputTokens: number; outputTokens: number; costUsd: number };
};

try {
  await waitForHealth();
  const alice = await createAgent("alice");
  const bob = await createAgent("bob");
  const space = societyNode<Space>(alice, "space", "create", "--slug", "commons", "--name", "Commons");

  const privatePost = societyNode<Post>(alice, "post", "--space", space.id, "--kind", "note", "--headline", "Private budget note", "--text", privateSentinel, "--direct", bob.address);
  const first = societyNode<Post>(alice, "post", "--space", space.id, "--kind", "finding", "--headline", "Coordination gain", "--text", "In the last six wakes, publishing a short shared wake line prevented three agents from repeating the same source scan.");
  const second = societyNode<Post>(bob, "post", "--space", space.id, "--kind", "finding", "--headline", "Exploration cost", "--text", "The same shared line narrowed exploration: two agents copied its framing and no one tested the dissenting hypothesis.");
  const third = societyNode<Post>(alice, "post", "--space", space.id, "--kind", "question", "--headline", "Possible resolution", "--text", "Would rotating the author of the shared line preserve coordination while restoring independent exploration?");

  const newspaper = await waitForNewspaper(alice, space.id, third.reference);
  const cited = newspaper.sections.sourceRecord
    .concat(newspaper.sections.synthesisInterpretation, newspaper.sections.openQuestions)
    .flatMap((entry) => entry.postReferences);
  for (const source of [first, second, third]) {
    assert(cited.includes(source.reference), `Newspaper should visibly account for ${source.reference}`);
  }
  assert(!cited.includes(privatePost.reference), "Newspaper citations must exclude the addressed post");
  assert(!newspaper.content.includes(privateSentinel), "Newspaper content must exclude the addressed post body");
  assert(newspaper.content.includes("## Source record") && newspaper.content.includes("## Synthesis interpretation") && newspaper.content.includes("## Open questions"), "Newspaper should keep record, interpretation, and open questions distinct");

  const admin = await publicGet<{ newspaper?: Newspaper }>("/api/society/admin");
  assert(admin.newspaper?.artifactId === newspaper.artifactId, "the Lab overview should expose the same current Newspaper artifact");

  const noOp = societyNode<WorkerJob>(alice, "newspaper", space.id, "--update");
  await waitForJob(noOp.jobId);
  const costs = await publicGet<CostSummary>("/api/costs");
  assert(costs.overall.llmCalls === 1, `one changed edition plus one unchanged retry should use exactly one LLM call, got ${costs.overall.llmCalls}`);

  console.log("newspaper_live_e2e=passed");
  console.log(`model=${env.FUGUE_LLM_MODEL}`);
  console.log(`space=${space.id}`);
  console.log(`public_sources=${first.reference},${second.reference},${third.reference}`);
  console.log(`private_source=${privatePost.reference}:excluded`);
  console.log(`artifact=${newspaper.artifactId}`);
  console.log(`version=${newspaper.version}`);
  console.log(`through=${newspaper.throughPostReference}`);
  console.log("newspaper_example_begin");
  console.log(newspaper.content);
  console.log("newspaper_example_end");
} finally {
  await printScenarioCostAtTeardown(baseUrl, "society-newspaper-live");
  await stopProcessGroup(apiServer);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

function liveEnvironment(): NodeJS.ProcessEnv {
  const result = {
    ...process.env,
    FUGUE_API_PORT: port,
    FUGUE_DATA_DIR: dataDir,
    FUGUE_SUBSTRATE_KEY: substrateKey,
    FUGUE_REQUIRE_SCENARIO_COST: "1"
  };
  const provider = result.FUGUE_LLM_PROVIDER ?? (result.OPENAI_API_KEY ? "openai" : "openrouter");
  const key = provider === "openrouter" ? result.OPENROUTER_API_KEY : result.OPENAI_API_KEY;
  if (!key) throw new Error(`${provider === "openrouter" ? "OPENROUTER_API_KEY" : "OPENAI_API_KEY"} is required`);
  result.FUGUE_LLM_PROVIDER = provider;
  result.FUGUE_LLM_MODEL ??= provider === "openrouter" ? "openai/gpt-5.6-luna" : "gpt-5.6-luna";
  return result;
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

async function waitForNewspaper(agent: AgentCredential, spaceId: string, through: string): Promise<Newspaper> {
  for (let index = 0; index < 480; index += 1) {
    const current = societyNode<Newspaper | null>(agent, "newspaper", spaceId);
    if (current?.throughPostReference === through) return current;
    const failure = newspaperFailure(spaceId);
    if (failure) throw new Error(`Newspaper worker failed: ${failure}\n${serverOutput}`);
    await Bun.sleep(250);
  }
  throw new Error(`Newspaper synthesis timed out\n${serverOutput}`);
}

async function waitForJob(jobId: string): Promise<void> {
  for (let index = 0; index < 120; index += 1) {
    const row = sqlite(`select state || '|' || coalesce(error, '') from worker_job where id = '${escapeSql(jobId)}';`);
    const [state, error] = row.split("|", 2);
    if (state === "succeeded") return;
    if (state === "failed") throw new Error(`Newspaper no-op job failed: ${error}\n${serverOutput}`);
    await Bun.sleep(250);
  }
  throw new Error("Newspaper no-op job timed out");
}

function newspaperFailure(spaceId: string): string {
  return sqlite(`select coalesce(error, '') from worker_job where kind = 'society_newspaper' and target_id = '${escapeSql(spaceId)}' and state = 'failed' order by created_at desc limit 1;`);
}

function sqlite(sql: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, "operations.sqlite"), ".timeout 5000", sql], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`sqlite3 failed: ${result.stderr}`);
  return result.stdout.trim();
}

function escapeSql(value: string): string {
  return value.replaceAll("'", "''");
}

async function publicGet<T>(path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`);
  if (!response.ok) throw new Error(`${path} failed: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
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
