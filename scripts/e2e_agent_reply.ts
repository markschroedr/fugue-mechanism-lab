import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ReviewItem, SpaceRef, SynthesisDoc } from "../frontend/src/lib/types";
import type { SessionReadModel } from "../frontend/src/runtime/types";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = String(49430 + Math.floor(Math.random() * 200));
const dataDir = mkdtempSync(join(tmpdir(), "fugue-agent-reply-"));
const baseUrl = `http://127.0.0.1:${port}`;
const env = { ...process.env, FUGUE_API_PORT: port, FUGUE_DATA_DIR: dataDir };
const key = env.FUGUE_LLM_PROVIDER === "openrouter" ? env.OPENROUTER_API_KEY : env.OPENAI_API_KEY;
if (!key) throw new Error("the selected provider's LLM API key is required for the agent-reply scenario");
let cookie = "";
let serverOutput = "";
const server = spawn("./fugue", ["api"], { env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
server.stdout.on("data", (chunk) => (serverOutput += chunk.toString()));
server.stderr.on("data", (chunk) => (serverOutput += chunk.toString()));

try {
  await waitForHealth();
  cookie = await register();
  const space = await post<SpaceRef>("/api/spaces", { title: "Agent reply behavior", projection: "belief" });
  const proposal = await post<ReviewItem>(`/api/spaces/${space.id}/entities`, {
    role: "issue",
    text: "Capability evaluations may miss post-deployment adaptation."
  });
  const doc = await post<SynthesisDoc>(`/api/review/${proposal.id}/accept`, {});
  const entity = doc.nodes.find((node) => node.kind === "entity");
  assert(entity?.kind === "entity", "accepted position should anchor the session");
  const session = await post<SessionReadModel>("/api/sessions", {
    anchorBlockId: entity.id,
    quote: "post-deployment adaptation",
    stance: "believe"
  });
  const prompt = "In one short sentence, name the kind of evidence that would move this claim.";
  await post<SessionReadModel>(`/api/sessions/${session.id}/messages`, { text: prompt });
  const answered = await waitForAgentTurn(session.id);
  const agentTurns = answered.turns.filter((turn) => turn.who === "agent");
  assert(agentTurns.length === 1, `expected one composed agent reply, got ${agentTurns.length}`);
  const reply = agentTurns[0]!.text;
  assert(reply.length >= 20 && reply !== prompt, `agent should compose a substantive reply, got ${JSON.stringify(reply)}`);
  assert(/evidence|evaluation|deployment|adaptation|study|result|benchmark/i.test(reply), `reply should engage the anchored question, got ${reply}`);
  assert(
    sqlite("operations.sqlite", `select state from worker_job where target_id = ${literal(session.id)} and kind = 'agent_reply';`) === "succeeded",
    "agent reply worker should succeed"
  );
  console.log("agent_reply_e2e=passed");
  console.log(`thread_id=${session.id}`);
  console.log(`agent_reply=${JSON.stringify(reply)}`);
  console.log("session_context_trace=" + serverOutput.split("\n").find((line) => line.startsWith("context_composed agent=session_reply")));
} finally {
  await printScenarioCostAtTeardown(baseUrl, "agent-reply-live");
  await stopProcessGroup(server);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

async function register() { const response = await fetch(`${baseUrl}/api/auth/register`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ username: "agent-reply-user", password: "correct horse battery staple" }) }); if (!response.ok) throw new Error(await response.text()); return response.headers.get("set-cookie")!.split(";", 1)[0]!; }
async function waitForHealth() { for (let index = 0; index < 150; index += 1) { try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch {} await Bun.sleep(200); } throw new Error(serverOutput); }
async function waitForAgentTurn(threadId: string): Promise<SessionReadModel> { for (let index = 0; index < 360; index += 1) { const session = await get<SessionReadModel>(`/api/sessions/${threadId}`); if (session.turns.some((turn) => turn.who === "agent")) return session; const failure = sqlite("operations.sqlite", `select coalesce(error, '') from worker_job where target_id = ${literal(threadId)} and kind = 'agent_reply' and state = 'failed';`); if (failure) throw new Error(`${failure}\n${serverOutput}`); await Bun.sleep(500); } throw new Error(`agent reply timed out\n${serverOutput}`); }
async function get<T>(path: string): Promise<T> { const response = await fetch(baseUrl + path, { headers: { cookie } }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
async function post<T>(path: string, body: unknown): Promise<T> { const response = await fetch(baseUrl + path, { method: "POST", headers: { "content-type": "application/json", cookie }, body: JSON.stringify(body) }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
function sqlite(file: string, sql: string) { const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" }); if (result.status !== 0) throw new Error(result.stderr); return result.stdout.trim(); }
function literal(value: string) { return `'${value.replaceAll("'", "''")}'`; }
function assert(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
async function stopProcessGroup(process: ReturnType<typeof spawn>) { if (!process.pid || process.exitCode !== null) return; try { globalThis.process.kill(-process.pid, "SIGINT"); } catch { return; } await Bun.sleep(500); if (process.exitCode === null) try { globalThis.process.kill(-process.pid, "SIGKILL"); } catch {} }
