import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn } from "bun";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const root = join(import.meta.dir, "..");
const dataDir = mkdtempSync(join(tmpdir(), "fugue-incremental-"));
const port = 50000 + Math.floor(Math.random() * 9000);
const baseUrl = `http://127.0.0.1:${port}`;
const env = { ...process.env, FUGUE_DATA_DIR: dataDir, FUGUE_API_PORT: String(port), FUGUE_LLM_PROVIDER: "openrouter" };
if (!env.OPENROUTER_API_KEY) throw new Error("OPENROUTER_API_KEY is required");

let serverOutput = "";
const server = spawn({
  cmd: ["cargo", "run", "-p", "fugue-server"],
  cwd: root,
  env,
  stdout: "pipe",
  stderr: "pipe"
});
void collect(server.stdout);
void collect(server.stderr);

try {
  await waitForHealth();
  const registration = await fetch(`${baseUrl}/api/auth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username: "incremental-e2e", password: "correct horse battery staple" })
  });
  assert(registration.ok, `registration failed: ${await registration.text()}`);
  const cookie = registration.headers.get("set-cookie")?.split(";", 1)[0];
  assert(cookie, "registration did not set a session cookie");

  const space = await post<{ id: string }>("/api/spaces", { title: "When does AI governance capacity bind?", projection: "belief" }, cookie);

  const seedAText = "Technical evaluation capacity can bind before capital because deployment creates adaptation and elicitation gaps.";
  const seedA = await propose(space.id, seedAText, cookie);
  const seedB = await propose(space.id, "Institutional adoption is a distinct bottleneck: incumbents can resist handoff even when technical control exists.", cookie);
  const seededDoc = await accept<Doc>(seedA.id, cookie);
  await accept(seedB.id, cookie);
  const seedEntityId = seededDoc.nodes.find((node) => node.text === seedAText)?.id;
  assert(seedEntityId, "accepted seed entity should be present in the returned document");
  const report1 = await waitForReport(space.id, 1);
  const initialCosts = await get<CostSummary>("/api/costs");
  assert(initialCosts.overall.llmCalls === 2, `rapid accepts should produce one router+synthesis attempt, got ${initialCosts.overall.llmCalls} calls`);

  const minor = await propose(space.id, "The Cedar-7 packaging audit adds a narrow, new consideration: evaluator access can lag demand by six months even when funding and institutional adoption are adequate. Preserve Cedar-7 in the reader-visible source record without changing the report's central technical-versus-institutional crux.", cookie);
  await accept(minor.id, cookie);
  const report2 = await waitForReport(space.id, 2);
  const route2 = { mode: report2.mode, reason: report2.routingReason ?? "" };
  assert(route2.reason.length > 0, "minor routing reason should be recorded in report history");
  assert(/Cedar-7/i.test(report2.content) && /evaluator/i.test(report2.content), "minor accepted consideration is not reader-visible");
  const selectiveRoutingCheck = route2.mode === "selective"
    ? "passed (router selected selective arm and server stored its result)"
    : "not applicable (router selected full arm)";

  const major = await propose(space.id, "New evidence overturns the framing: technical and institutional bottlenecks are downstream of a three-camp disagreement over whether governance should optimize centralized evaluation, pluralistic jurisdictional competition, or an international deployment moratorium. The report must reorganize around these incompatible objectives rather than retain its prior two-bottleneck frame.", cookie);
  await accept(major.id, cookie);
  const report3 = await waitForReport(space.id, 3);
  const route3 = { mode: report3.mode, reason: report3.routingReason ?? "" };
  assert(route3.reason.length > 0, "major routing reason should be recorded in report history");

  await request(`/api/entities/${seedEntityId}`, "PATCH", {
    text: `${seedAText} This accepted wording update clarifies that the constraint is evaluator access, not evaluator demand.`
  }, cookie);
  const report4 = await waitForReport(space.id, 4);
  const history = await get<ReportVersion[]>(`/api/spaces/${space.id}/report/history`);
  const autoEditVersion = history.find((version) => version.version === report4.version);
  assert(autoEditVersion?.causedBy.some((cause) => cause.kind === "entity_text_updated"), "auto-accepted text edit should be recorded in the report cause chain");
  assert(report4.routingReason, "auto-accepted text edit should have a routing reason");

  const costs = await get<CostSummary>("/api/costs");
  const spaceCosts = costs.spaces.find((entry) => entry.spaceId === space.id);
  assert(costs.overall.inputTokens > 0 && costs.overall.outputTokens > 0 && costs.overall.costUsd > 0, "overall cost fields should be populated");
  assert(spaceCosts && spaceCosts.costUsd > 0, "space cost total should be populated");
  const callRows = costs.overall.llmCalls;
  assert(callRows >= 7, `expected routing across four attempts plus substantive synthesis calls, got ${callRows}`);

  console.log("incremental_synthesis_e2e=passed");
  console.log(`configured_model=${(await get<{ model?: string }>("/api/health")).model}`);
  console.log(`served_models=${costs.servedModels.join(",")}`);
  console.log("debounced_accepts=2,synthesis_jobs=1");
  console.log(`minor_route=${route2.mode}`);
  console.log(`minor_reason=${route2.reason}`);
  console.log(`minor_reader_outcome=passed (Cedar-7 packaging audit and evaluator constraint visible)`);
  console.log(`minor_selective_routing_check=${selectiveRoutingCheck}`);
  console.log(`major_route=${route3.mode}`);
  console.log(`major_reason=${route3.reason}`);
  console.log(`auto_edit_route=${report4.mode}`);
  console.log(`auto_edit_reason=${report4.routingReason}`);
  console.log(`auto_edit_cause=${autoEditVersion!.causedBy.map((cause) => cause.kind).join(",")}`);
  console.log(`report_versions=${report1.version},${report2.version},${report3.version},${report4.version}`);
  console.log(`llm_call_records=${callRows}`);
  console.log(`space_tokens_in=${spaceCosts!.inputTokens}`);
  console.log(`space_tokens_out=${spaceCosts!.outputTokens}`);
  console.log(`space_cost_usd=${spaceCosts!.costUsd.toFixed(8)}`);
  console.log(`overall_cost_usd=${costs.overall.costUsd.toFixed(8)}`);
  console.log("report_tail=");
  console.log(report4.content.slice(-1000));
} finally {
  await printScenarioCostAtTeardown(baseUrl, "incremental-synthesis-live");
  server.kill();
  await server.exited;
}

type Report = { version: number; content: string; mode: "full" | "selective"; routingReason?: string };
type ReportVersion = Report & { causedBy: Array<{ kind: string }> };
type Doc = { nodes: Array<{ id: string; text?: string }> };
type CostSummary = { overall: Totals; spaces: Array<Totals & { spaceId: string }>; servedModels: string[] };
type Totals = { llmCalls: number; inputTokens: number; outputTokens: number; costUsd: number };

async function propose(spaceId: string, text: string, cookie: string) {
  return post<{ id: string }>(`/api/spaces/${spaceId}/entities`, { role: "issue", text }, cookie);
}
async function accept<T = unknown>(id: string, cookie: string) {
  return post<T>(`/api/review/${id}/accept`, {}, cookie);
}
async function post<T = unknown>(path: string, body: unknown, cookie: string): Promise<T> {
  return request<T>(path, "POST", body, cookie);
}
async function request<T = unknown>(path: string, method: string, body: unknown, cookie: string): Promise<T> {
  const response = await fetch(baseUrl + path, { method, headers: { "content-type": "application/json", cookie }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}
async function get<T>(path: string): Promise<T> {
  const response = await fetch(baseUrl + path);
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}
async function waitForHealth() {
  for (let i = 0; i < 120; i++) {
    try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch {}
    await sleep(250);
  }
  throw new Error(`server did not start:\n${serverOutput}`);
}
async function waitForReport(spaceId: string, version: number): Promise<Report> {
  for (let i = 0; i < 800; i++) {
    const report = await get<Report | null>(`/api/spaces/${spaceId}/report`);
    if (report && report.version >= version) return report;
    await sleep(500);
  }
  throw new Error(`report v${version} timed out:\n${serverOutput}`);
}
function sleep(ms: number) { return new Promise((resolve) => setTimeout(resolve, ms)); }
function assert(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
async function collect(stream: ReadableStream<Uint8Array> | null) {
  if (!stream) return;
  for await (const chunk of stream) serverOutput += new TextDecoder().decode(chunk);
}
