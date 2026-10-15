import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const root = join(import.meta.dir, "..");
const dataDir = mkdtempSync(join(tmpdir(), "fugue-synthesis-crash-"));
const port = 52500 + Math.floor(Math.random() * 500);
const baseUrl = `http://127.0.0.1:${port}`;
const baseEnv = { ...process.env, FUGUE_DATA_DIR: dataDir, FUGUE_API_PORT: String(port) };
const key = baseEnv.FUGUE_LLM_PROVIDER === "openrouter" ? baseEnv.OPENROUTER_API_KEY : baseEnv.OPENAI_API_KEY;
if (!key) throw new Error("the selected provider's LLM API key is required");
let cookie = "";
let firstOutput = "";
let secondOutput = "";
let first: ReturnType<typeof spawn> | null = spawnServer({ ...baseEnv, FUGUE_TEST_CRASH_AFTER_SYNTHESIS_COMMIT: "1" }, (text) => firstOutput += text);

try {
  await waitForHealth(() => firstOutput);
  cookie = await register();
  const space = await post<{ id: string }>("/api/spaces", { title: "Crash-safe record", projection: "belief" });
  const review = await post<{ id: string }>(`/api/spaces/${space.id}/entities`, {
    role: "issue",
    text: "Selective optimism may underweight institutional and coordination failure modes."
  });
  await post(`/api/review/${review.id}/accept`, {});

  await waitForExit(first);
  const jobId = sqlite("operations.sqlite", "select id from worker_job where kind = 'synthesize' order by created_at desc limit 1;");
  const artifactId = sqlite("operations.sqlite", `select effect_artifact_id from worker_job where id = ${literal(jobId)};`);
  assert(artifactId.length > 0, "report effect pointer must commit before the crash");
  assert(sqlite("operations.sqlite", `select state from worker_job where id = ${literal(jobId)};`) === "running", "crash must occur before job completion");
  const callsBefore = Number(sqlite("operations.sqlite", `select coalesce(sum(json_array_length(llm_calls_json)),0) from worker_attempt where job_id = ${literal(jobId)};`));

  first = null;
  const second = spawnServer(baseEnv, (text) => secondOutput += text);
  try {
    await waitForHealth(() => secondOutput);
    await waitForJob(jobId, "succeeded");
    const history = await get<Array<{ version: number; artifactId: string; causedBy: Array<{ eventId: string }> }>>(`/api/spaces/${space.id}/report/history`);
    const reportCount = Number(sqlite("core.sqlite", `select count(*) from artifact where json_extract(metadata_json, '$.artifactType') = 'synthesisReport' and json_extract(metadata_json, '$.spaceId') = ${literal(space.id)};`));
    const eventCount = Number(sqlite("events.sqlite", "select count(*) from event where kind = 'synthesis_report_written';"));
    const causeCount = history.flatMap((version) => version.causedBy).length;
    const distinctCauseCount = new Set(history.flatMap((version) => version.causedBy.map((cause) => cause.eventId))).size;
    const attempts = sqlite("operations.sqlite", `select group_concat(state, ',') from (select state from worker_attempt where job_id = ${literal(jobId)} order by started_at);`);
    const callsAfter = Number(sqlite("operations.sqlite", `select coalesce(sum(json_array_length(llm_calls_json)),0) from worker_attempt where job_id = ${literal(jobId)};`));
    const routingReason = sqlite("core.sqlite", `select coalesce(json_extract(metadata_json, '$.routingReason'),'') from artifact where id = ${literal(artifactId)};`);

    assert(history.length === 1 && history[0]?.version === 1 && history[0]?.artifactId === artifactId, "recovery must preserve exactly report v1");
    assert(reportCount === 1 && eventCount === 1, "report artifact and write event must each exist exactly once");
    assert(causeCount === 1 && distinctCauseCount === 1, "the accepted cause must appear exactly once");
    assert(attempts === "failed,succeeded", `interrupted and recovery attempts must be terminal, got ${attempts}`);
    assert(callsAfter === callsBefore, "recovery must not make another LLM call or change cost accounting");
    assert(routingReason.length > 0, "automatic synthesis must preserve its routing reason");

    const noOpJobId = "job-e2e-empty-auto";
    sqlite("operations.sqlite", `insert into worker_job (id, kind, state, payload_json, target_id, created_at, updated_at) values (${literal(noOpJobId)}, 'synthesize', 'queued', ${literal(JSON.stringify({ spaceId: space.id, mode: "full", instruction: null, auto: true }))}, ${literal(space.id)}, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));`);
    await waitForJob(noOpJobId, "succeeded");
    const noOpCalls = Number(sqlite("operations.sqlite", `select coalesce(sum(json_array_length(llm_calls_json)),0) from worker_attempt where job_id = ${literal(noOpJobId)};`));
    const reportCountAfterNoOp = Number(sqlite("core.sqlite", `select count(*) from artifact where json_extract(metadata_json, '$.artifactType') = 'synthesisReport' and json_extract(metadata_json, '$.spaceId') = ${literal(space.id)};`));
    assert(noOpCalls === 0 && reportCountAfterNoOp === 1, "empty automatic cause window must succeed without calls or a new report");

    console.log("synthesis_crash_idempotency_e2e=passed");
    console.log(`space_id=${space.id}`);
    console.log(`job_id=${jobId}`);
    console.log(`effect_artifact_id=${artifactId}`);
    console.log(`report_versions=${history.map((item) => item.version).join(",")}`);
    console.log(`cause_count=${causeCount}`);
    console.log(`attempt_states=${attempts}`);
    console.log(`llm_calls_before_restart=${callsBefore}`);
    console.log(`llm_calls_after_restart=${callsAfter}`);
    console.log(`routing_reason=${routingReason}`);
    console.log(`empty_auto_noop_calls=${noOpCalls}`);
    console.log(`empty_auto_noop_report_count=${reportCountAfterNoOp}`);
    console.log(`crash_exit_code=86`);
  } finally {
    await printScenarioCostAtTeardown(baseUrl, "synthesis-crash-live");
    await stopProcessGroup(second);
  }
} finally {
  if (first && first.exitCode === null) await stopProcessGroup(first);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

function spawnServer(env: NodeJS.ProcessEnv, output: (text: string) => void) {
  const child = spawn("./fugue", ["api"], { cwd: root, env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
  child.stdout.on("data", (chunk) => output(chunk.toString()));
  child.stderr.on("data", (chunk) => output(chunk.toString()));
  return child;
}
async function waitForExit(child: ReturnType<typeof spawn>) {
  if (child.exitCode === null) await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`server did not crash after report commit\n${firstOutput}`)), 360_000);
    child.once("exit", () => { clearTimeout(timer); resolve(); });
  });
  assert(child.exitCode === 86, `expected crash hook exit 86, got ${child.exitCode}\n${firstOutput}`);
}
async function waitForHealth(output: () => string) {
  for (let i = 0; i < 150; i++) { try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch {} await Bun.sleep(200); }
  throw new Error(`API did not start\n${output()}`);
}
async function waitForJob(id: string, state: string) {
  for (let i = 0; i < 200; i++) { if (sqlite("operations.sqlite", `select state from worker_job where id = ${literal(id)};`) === state) return; await Bun.sleep(100); }
  throw new Error(`job ${id} did not reach ${state}\n${secondOutput}`);
}
async function register() { const response = await fetch(`${baseUrl}/api/auth/register`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ username: "crash-e2e", password: "correct horse battery staple" }) }); if (!response.ok) throw new Error(await response.text()); return response.headers.get("set-cookie")!.split(";", 1)[0]!; }
async function get<T>(path: string): Promise<T> { const response = await fetch(baseUrl + path, { headers: { cookie } }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
async function post<T = unknown>(path: string, body: unknown): Promise<T> { const response = await fetch(baseUrl + path, { method: "POST", headers: { "content-type": "application/json", cookie }, body: JSON.stringify(body) }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
function sqlite(file: string, sql: string) { const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" }); if (result.status !== 0) throw new Error(result.stderr); return result.stdout.trim(); }
function literal(value: string) { return `'${value.replaceAll("'", "''")}'`; }
function assert(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
async function stopProcessGroup(child: ReturnType<typeof spawn>) { if (!child.pid || child.exitCode !== null) return; try { process.kill(-child.pid, "SIGINT"); } catch { return; } await Bun.sleep(600); if (child.exitCode === null) try { process.kill(-child.pid, "SIGKILL"); } catch {} }
