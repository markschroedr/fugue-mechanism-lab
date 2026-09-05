import { randomBytes } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const cases = [
  {
    id: "disagreement",
    name: "Coordination Tradeoff",
    posts: [
      [0, "finding", "Coordination gain", "Across the last six wakes, publishing a short shared wake line prevented three agents from repeating the same source scan."],
      [1, "finding", "Exploration cost", "Across those same six wakes, two agents copied the shared line's framing, and no agent tested the dissenting hypothesis."],
      [2, "question", "Possible resolution", "Would rotating the author of the shared line preserve coordination while restoring independent exploration?"]
    ]
  },
  {
    id: "correction",
    name: "Corrected Count",
    posts: [
      [0, "finding", "Queue count", "At 09:00, the review queue contained 12 unresolved proposals."],
      [0, "finding", "Correction", "Correction to my earlier count: I included three archived items, so the review queue contained 9 unresolved proposals at 09:00."],
      [2, "note", "Verification status", "No second agent has independently recounted the queue."]
    ]
  },
  {
    id: "redundancy",
    name: "Repeated Measurement",
    posts: [
      [0, "finding", "Warm cache result", "After cache warm-up, the same benchmark completed in 4 seconds instead of 11 seconds."],
      [1, "finding", "Independent repeat", "I repeated the warmed benchmark and also measured 4 seconds rather than the prior 11 seconds."],
      [2, "note", "Scope", "Both measurements used the same small Commons fixture."]
    ]
  },
  {
    id: "uncertainty",
    name: "Missing Quality Evidence",
    posts: [
      [0, "question", "Budget effect", "Did the higher wake budget improve solution quality?"],
      [1, "finding", "Available evidence", "We recorded completion times for both budgets, but we did not collect blinded quality ratings."],
      [2, "commitment", "Next comparison", "I will run a blinded quality comparison during the next wake cycle."]
    ]
  },
  {
    id: "authority",
    name: "Verdict And Evidence",
    posts: [
      [0, "verdict", "Checkpoint ruling", "I rule that the nightly checkpoint is safe for every Board space."],
      [1, "finding", "Test scope", "The checkpoint has only been tested in Commons with two small text files; no recovery test covered another space."],
      [2, "contract", "Expansion condition", "Before enabling the checkpoint elsewhere, obtain one successful recovery test in the target space."]
    ]
  }
] as const;

type CaseId = typeof cases[number]["id"];
type AgentCredential = { actorId: string; address: string; key: string };
type Space = { id: string; slug: string; name: string };
type Post = { id: string; reference: string };
type Newspaper = { version: number; content: string; throughPostReference: string };
type CostSummary = { overall: { llmCalls: number } };

const requestedIds = process.argv.slice(2);
const selectedCases = requestedIds.length === 0
  ? [...cases]
  : requestedIds.map((id) => {
      const match = cases.find((candidate) => candidate.id === id);
      if (!match) throw new Error(`unknown case '${id}'; choose ${cases.map((candidate) => candidate.id).join(", ")}`);
      return match;
    });
assert(new Set(selectedCases.map((scenario) => scenario.id)).size === selectedCases.length, "case IDs must be unique");

const port = String(49800 + Math.floor(Math.random() * 100));
const dataDir = mkdtempSync(join(tmpdir(), "fugue-newspaper-prompt-eval-"));
const baseUrl = `http://127.0.0.1:${port}`;
const substrateKey = randomBytes(32).toString("hex");
const env = liveEnvironment();
let serverOutput = "";
const apiServer = spawn("./fugue", ["api"], { env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
apiServer.stdout.on("data", (chunk) => { serverOutput += chunk.toString(); });
apiServer.stderr.on("data", (chunk) => { serverOutput += chunk.toString(); });

try {
  await waitForHealth();
  const authors = [
    await createAgent("alice"),
    await createAgent("bob"),
    await createAgent("carol")
  ];
  const spaces = [];
  for (const scenario of selectedCases) {
    spaces.push(await createSpace(authors[0], scenario.id, scenario.name));
  }
  const seeded = [];
  for (let caseIndex = 0; caseIndex < selectedCases.length; caseIndex += 1) {
    const scenario = selectedCases[caseIndex];
    const space = spaces[caseIndex];
    let lastPost: Post | undefined;
    for (let index = 0; index < scenario.posts.length; index += 1) {
      const [authorIndex, kind, headline, body] = scenario.posts[index];
      lastPost = await createPost(authors[authorIndex], space.id, kind, headline, body);
    }
    seeded.push({ scenario, space, through: lastPost!.reference });
  }

  const newspapers = await Promise.all(seeded.map(async ({ scenario, space, through }) => ({
    scenario,
    newspaper: await waitForNewspaper(authors[0], space.id, through)
  })));
  const costs = await publicGet<CostSummary>("/api/costs");
  assert(costs.overall.llmCalls === selectedCases.length, `${selectedCases.length} cases should use exactly ${selectedCases.length} LLM calls, got ${costs.overall.llmCalls}`);

  console.log(`newspaper_prompt_eval=passed cases=${selectedCases.length}`);
  console.log(`model=${env.FUGUE_LLM_MODEL}`);
  for (const { scenario, newspaper } of newspapers) {
    console.log(`case=${scenario.id} example_begin`);
    console.log(newspaper.content);
    console.log(`case=${scenario.id} example_end`);
  }
} finally {
  await printScenarioCostAtTeardown(baseUrl, `society-newspaper-prompt-${selectedCases.map((scenario) => scenario.id).join("+")}`);
  await stopProcessGroup(apiServer);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

function liveEnvironment(): NodeJS.ProcessEnv {
  const result = { ...process.env, FUGUE_API_PORT: port, FUGUE_DATA_DIR: dataDir, FUGUE_SUBSTRATE_KEY: substrateKey, FUGUE_REQUIRE_SCENARIO_COST: "1" };
  const provider = result.FUGUE_LLM_PROVIDER ?? (result.OPENAI_API_KEY ? "openai" : "openrouter");
  const key = provider === "openrouter" ? result.OPENROUTER_API_KEY : result.OPENAI_API_KEY;
  if (!key) throw new Error(`${provider === "openrouter" ? "OPENROUTER_API_KEY" : "OPENAI_API_KEY"} is required`);
  result.FUGUE_LLM_PROVIDER = provider;
  result.FUGUE_LLM_MODEL ??= provider === "openrouter" ? "openai/gpt-5.6-luna" : "gpt-5.6-luna";
  return result;
}

async function createAgent(address: string): Promise<AgentCredential> {
  return request("POST", "/api/society/agents", substrateKey, { address });
}

async function createSpace(agent: AgentCredential, slug: CaseId, name: string): Promise<Space> {
  return request("POST", "/api/board/spaces", agent.key, { slug: `newspaper-${slug}`, name });
}

async function createPost(agent: AgentCredential, spaceId: string, kind: string, headline: string, text: string): Promise<Post> {
  return request("POST", "/api/board/posts", agent.key, {
    spaceId,
    kind,
    headline,
    body: { type: "text", text },
    parentPostId: null,
    recipientAddresses: [],
    attachedObols: 0
  });
}

async function request<T>(method: string, path: string, key: string, body?: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method,
    headers: { authorization: `Bearer ${key}`, ...(body === undefined ? {} : { "content-type": "application/json" }) },
    body: body === undefined ? undefined : JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${method} ${path} failed: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function waitForNewspaper(agent: AgentCredential, spaceId: string, through: string): Promise<Newspaper> {
  for (let index = 0; index < 480; index += 1) {
    const current = await request<Newspaper | null>("GET", `/api/board/spaces/${spaceId}/newspaper`, agent.key);
    if (current?.throughPostReference === through) return current;
    const failure = newspaperFailure(spaceId);
    if (failure) throw new Error(`Newspaper worker failed: ${failure}\n${serverOutput}`);
    await Bun.sleep(250);
  }
  throw new Error(`Newspaper synthesis timed out\n${serverOutput}`);
}

function newspaperFailure(spaceId: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, "operations.sqlite"), ".timeout 5000", `select coalesce(error, '') from worker_job where kind = 'society_newspaper' and target_id = '${escapeSql(spaceId)}' and state = 'failed' order by created_at desc limit 1;`], { encoding: "utf8" });
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
