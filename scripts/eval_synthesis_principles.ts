import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const root = join(import.meta.dir, "..");
const dataDir = mkdtempSync(join(tmpdir(), "fugue-principles-eval-"));
const port = 51000 + Math.floor(Math.random() * 7000);
const baseUrl = `http://127.0.0.1:${port}`;
const threadUrl = process.env.FUGUE_EVAL_THREAD_URL ?? "https://www.lesswrong.com/posts/BBd2EJywf2xXftyFn/selective-optimism-a-critique-of-ai-2040";
const env = { ...process.env, FUGUE_DATA_DIR: dataDir, FUGUE_API_PORT: String(port) };
const key = env.FUGUE_LLM_PROVIDER === "openrouter" ? env.OPENROUTER_API_KEY : env.OPENAI_API_KEY;
if (!key) throw new Error("the selected provider's LLM API key is required for the behavioral eval");

let cookie = "";
let serverOutput = "";
const server = spawn("./fugue", ["api"], { cwd: root, env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
server.stdout.on("data", (chunk) => (serverOutput += chunk.toString()));
server.stderr.on("data", (chunk) => (serverOutput += chunk.toString()));

try {
  await waitForHealth();
  cookie = await register();
  const costStart = await get<CostSummary>("/api/costs");
  const space = await post<{ id: string }>("/api/spaces", { title: "Selective Optimism: synthesis-principles evaluation", projection: "belief" });
  const thread = await post<InputReceipt>(`/api/spaces/${space.id}/input`, { kind: "text", content: threadUrl });
  const threadReview = await waitForReview(thread.artifactId);
  const baselineDoc = await post<Doc>(`/api/review/${threadReview.id}/accept`, {});
  const baseline = await waitForReport(space.id, 1);
  assertQualityStructure(baseline.content);
  const existing = baselineDoc.nodes.find((node) => node.kind === "entity" && node.text)?.text;
  assert(existing, "real thread ingest did not produce an existing position");
  const explicitHolders = baselineDoc.nodes.flatMap((node) => node.holders ?? []).map((holder) => holder.actorName);
  assert(explicitHolders.some((name) => boundedSection(baseline.content, "## Source record").includes(name)), "Source record did not preserve any explicit harvested attribution");
  console.log("attribution_assertion=passed (named holder backed by harvested stance)");

  const contradictionText = `The Kepler side directly contradicts the Atlas side. Atlas holds the accepted position that “${existing}”. Kepler rejects that exact conclusion: AI 2040 should not distinguish forecasting from recommendation, because every favorable scenario element is intentionally both a recommendation and a realistic forecast; the optimistic format does not obscure which outcomes are desirable, and the handoff is neither merely aspirational nor made to look more plausible by presentation. Atlas says the distinction is necessary; Kepler says it is unnecessary. This is a direct live incompatibility, not an additional caution or support for Atlas.`;
  console.log(`contradiction_input=${contradictionText}`);
  const contradiction = await inject(space.id, contradictionText, baseline.version);
  assert(["new-tension", "weakened"].includes(contradiction.classification), `contradiction was classified ${contradiction.classification}`);
  const cruxes = section(contradiction.report.content, "## Cruxes");
  assert(/Kepler/i.test(cruxes) && /Atlas/i.test(cruxes), "Cruxes must reference both controlled Kepler and Atlas sides");
  console.log(`contradiction_classification=${contradiction.classification}`);
  console.log("contradiction_assertions=passed (classification + two-sided Cruxes section)");

  const supportingText = "The Zephyr rehearsal evidence strengthens Atlas's case that Plan A merits serious evaluation: quarterly simulator drills, role-specific checklists, and recorded intervention walkthroughs let rotating teams retain rare-intervention proficiency despite infrequent live use. This supports feasibility without resolving or opposing either side of the current Kepler crux.";
  console.log(`supporting_input=${supportingText}`);
  const orthogonal = await inject(space.id, supportingText, contradiction.report.version);
  assert(orthogonal.classification === "strengthened", `orthogonal consideration was classified ${orthogonal.classification}`);
  assert(/Zephyr/i.test(boundedSection(orthogonal.report.content, "## Source record")), "source-supported Zephyr consideration did not appear in Source record");
  assert(boundedSection(orthogonal.report.content, "## Synthesis interpretation").trim().length > 0, "Synthesis interpretation section is empty");
  console.log(`supporting_classification=${orthogonal.classification}`);
  console.log("supporting_assertions=passed (classification + visible new content)");

  const duplicateText = boundedSection(orthogonal.report.content, "## Source record")
    .split("\n")
    .map((line) => line.trim())
    .find((line) => /Zephyr/i.test(line) && line.length >= 80);
  assert(duplicateText, "the strengthened report did not expose a Zephyr record to repeat exactly");
  console.log(`duplicate_input=${duplicateText}`);
  const duplicate = await inject(space.id, duplicateText, orthogonal.report.version);
  assert(duplicate.classification === "redundant", `near-duplicate was classified ${duplicate.classification}`);
  assert(duplicate.report.content === orthogonal.report.content, "redundant input changed reader-visible report bytes");
  console.log(`duplicate_classification=${duplicate.classification}`);
  console.log("duplicate_diff_bytes=0");
  console.log("duplicate_assertions=passed (redundant + byte-identical report)");

  cookie = await register("corrigible_skeptic");
  const weakeningText = "The Orion field audit weakens the Zephyr feasibility support without asserting Plan A's negation or creating a rival objective. Teams scored well on quarterly simulator drills and recorded walkthroughs, but during unannounced high-fidelity exercises their rotating personnel failed to transfer those rehearsed routines to rare interventions. This does not prove the system infeasible, but it undercuts Zephyr's load-bearing assumption that simulator retention transfers to real intervention competence.";
  console.log(`weakening_input=${weakeningText}`);
  const weakened = await inject(space.id, weakeningText, duplicate.report.version);
  assert(weakened.classification === "weakened", `load-bearing-assumption control was classified ${weakened.classification}`);
  assert(/Zephyr/i.test(weakened.reason) && /(weaken|undercut|less credible|challenge)/i.test(weakened.reason), `weakened label contradicted its reason: ${weakened.reason}`);
  const weakenedRecord = boundedSection(weakened.report.content, "## Source record");
  assert(/corrigible_skeptic/.test(weakenedRecord), "native input was not attributed to corrigible_skeptic in Source record");
  assert(/Orion/.test(weakenedRecord), "Orion weakening evidence is not visible in Source record");
  assert(weakened.report.content !== duplicate.report.content, "weakened input produced no reader-visible report movement");
  assertQualityStructure(weakened.report.content);
  console.log(`weakened_classification=${weakened.classification}`);
  console.log("weakened_assertions=passed (assumption undermined + native author + visible movement + markdown boundaries)");

  const costs = await get<CostSummary>("/api/costs");
  const spaceCosts = costs.spaces.find((entry) => entry.spaceId === space.id);
  assert(spaceCosts && spaceCosts.llmCalls > 0, "eval cost total missing");
  const costDelta = subtractCosts(costs, costStart);
  console.log("synthesis_principles_eval=passed");
  console.log(`configured_model=${(await get<{ model?: string }>("/api/health")).model}`);
  console.log(`served_models=${costs.servedModels.join(",")}`);
  console.log(`thread_url=${threadUrl}`);
  console.log(`report_versions=${baseline.version},${contradiction.report.version},${orthogonal.report.version},${duplicate.report.version},${weakened.report.version}`);
  console.log("cost_report_begin");
  for (const stage of ["ingest_extraction", "classification", "synthesis"]) {
    const totals = costDelta.stages.find((entry) => entry.stage === stage) ?? zeroCosts(stage);
    console.log(`cost_stage=${stage} llm_calls=${totals.llmCalls} tokens_in=${totals.inputTokens} tokens_out=${totals.outputTokens} dollars=${totals.costUsd.toFixed(8)}`);
  }
  console.log(`cost_total llm_calls=${costDelta.overall.llmCalls} tokens_in=${costDelta.overall.inputTokens} tokens_out=${costDelta.overall.outputTokens} dollars=${costDelta.overall.costUsd.toFixed(8)}`);
  console.log("cost_report_end");
  console.log("report_tail=");
  console.log(weakened.report.content.slice(-1600));
  console.log("composed_context_traces=");
  console.log([...new Set(serverOutput.split("\n").filter((line) => line.startsWith("context_composed ")))].join("\n"));
} finally {
  await printScenarioCostAtTeardown(baseUrl, "synthesis-principles-eval");
  await stopProcessGroup(server);
  if (process.env.FUGUE_KEEP_EVAL_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

type InputReceipt = { artifactId: string; jobId: string };
type ReviewItem = { id: string; sourceArtifactId?: string };
type Doc = { nodes: Array<{ id: string; kind: string; text?: string; holders?: Array<{ actorName: string }> }> };
type Report = { version: number; content: string };
type Cause = { eventId: string; kind: string; source?: { artifactId: string }; changeClassification?: string; classificationReason?: string };
type ReportVersion = Report & { causedBy: Cause[] };
type Totals = { llmCalls: number; inputTokens: number; outputTokens: number; costUsd: number };
type CostSummary = { overall: Totals; spaces: Array<Totals & { spaceId: string }>; stages: Array<Totals & { stage: string }>; servedModels: string[] };

function zeroCosts(stage: string): Totals & { stage: string } {
  return { stage, llmCalls: 0, inputTokens: 0, outputTokens: 0, costUsd: 0 };
}
function subtractTotals(after: Totals, before: Totals): Totals {
  return {
    llmCalls: after.llmCalls - before.llmCalls,
    inputTokens: after.inputTokens - before.inputTokens,
    outputTokens: after.outputTokens - before.outputTokens,
    costUsd: after.costUsd - before.costUsd
  };
}
function subtractCosts(after: CostSummary, before: CostSummary) {
  const stages = after.stages.map((entry) => {
    const prior = before.stages.find((candidate) => candidate.stage === entry.stage) ?? zeroCosts(entry.stage);
    return { stage: entry.stage, ...subtractTotals(entry, prior) };
  });
  return { overall: subtractTotals(after.overall, before.overall), stages };
}

async function inject(spaceId: string, content: string, previousVersion: number) {
  const receipt = await post<InputReceipt>(`/api/spaces/${spaceId}/input`, { kind: "text", content });
  const review = await waitForReview(receipt.artifactId);
  await post(`/api/review/${review.id}/accept`, {});
  const report = await waitForReport(spaceId, previousVersion + 1);
  assertQualityStructure(report.content);
  const history = await get<ReportVersion[]>(`/api/spaces/${spaceId}/report/history`);
  const version = history.find((item) => item.version === report.version);
  assert(version, `report history missing v${report.version}`);
  const matchingCauses = version.causedBy.filter((item) => item.source?.artifactId === receipt.artifactId);
  assert(matchingCauses.length === 1, `v${report.version} has ${matchingCauses.length} causes for injected artifact ${receipt.artifactId}`);
  const cause = matchingCauses[0];
  assert(cause?.changeClassification, `v${report.version} did not record the injected cause classification`);
  console.log(`classification_reason=${cause.classificationReason ?? "missing"}`);
  return { report, classification: cause.changeClassification, reason: cause.classificationReason ?? "" };
}

async function waitForReview(artifactId: string): Promise<ReviewItem> {
  for (let i = 0; i < 600; i++) {
    const item = (await get<ReviewItem[]>("/api/review")).find((candidate) => candidate.sourceArtifactId === artifactId);
    if (item) return item;
    throwOnWorkerFailure(artifactId);
    await Bun.sleep(500);
  }
  throw new Error(`review timed out for ${artifactId}\n${serverOutput}`);
}
async function waitForReport(spaceId: string, version: number): Promise<Report> {
  for (let i = 0; i < 800; i++) {
    const report = await get<Report | null>(`/api/spaces/${spaceId}/report`);
    if (report && report.version >= version) return report;
    const failure = sqlite("operations.sqlite", "select coalesce(group_concat(error, '\\n'), '') from worker_job where state = 'failed';");
    if (failure) throw new Error(`worker failed: ${failure}\n${serverOutput}`);
    await Bun.sleep(500);
  }
  throw new Error(`report v${version} timed out\n${serverOutput}`);
}
function throwOnWorkerFailure(artifactId: string) {
  const failure = sqlite("operations.sqlite", `select coalesce(error, '') from worker_job where json_extract(payload_json, '$.artifactId') = ${literal(artifactId)} and state = 'failed';`);
  if (failure) throw new Error(`ingest failed: ${failure}\n${serverOutput}`);
}
async function register(username = "principles-eval") {
  const response = await fetch(`${baseUrl}/api/auth/register`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ username, password: "correct horse battery staple" }) });
  assert(response.ok, `registration failed: ${await response.text()}`);
  const value = response.headers.get("set-cookie")?.split(";", 1)[0];
  assert(value, "registration did not set cookie");
  return value;
}
async function waitForHealth() {
  for (let i = 0; i < 150; i++) {
    try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch { /* starting */ }
    await Bun.sleep(200);
  }
  throw new Error(`server did not start\n${serverOutput}`);
}
async function get<T>(path: string): Promise<T> {
  const response = await fetch(baseUrl + path, { headers: { cookie } });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}
async function post<T = unknown>(path: string, body: unknown): Promise<T> {
  const response = await fetch(baseUrl + path, { method: "POST", headers: { "content-type": "application/json", cookie }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}
function markdownHeadings(content: string) {
  let offset = 0;
  let fenced = false;
  const headings: Array<{ text: string; offset: number }> = [];
  for (const line of content.split(/(?<=\n)/)) {
    const text = line.replace(/\r?\n$/, "");
    if (/^\s*(```|~~~)/.test(text)) fenced = !fenced;
    else if (!fenced && /^## [^#].*$/.test(text)) headings.push({ text, offset });
    offset += line.length;
  }
  return headings;
}
function assertQualityStructure(content: string) {
  const required = ["## Source record", "## Synthesis interpretation", "## Cruxes"];
  const headings = markdownHeadings(content).map((heading) => heading.text);
  assert(JSON.stringify(headings) === JSON.stringify(required), `report markdown sections were ${headings.join(", ")}`);
}
function section(content: string, heading: string) { return content.slice(markdownHeadings(content).find((item) => item.text === heading)?.offset ?? -1); }
function boundedSection(content: string, heading: string) {
  const headings = markdownHeadings(content);
  const index = headings.findIndex((item) => item.text === heading);
  assert(index >= 0, `missing ${heading}`);
  const start = headings[index]!.offset + heading.length;
  return content.slice(start, headings[index + 1]?.offset ?? content.length);
}
function sqlite(file: string, sql: string) {
  const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(result.stderr);
  return result.stdout.trim();
}
function literal(value: string) { return `'${value.replaceAll("'", "''")}'`; }
function assert(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
async function stopProcessGroup(process: ReturnType<typeof spawn>) {
  if (!process.pid || process.exitCode !== null) return;
  try { globalThis.process.kill(-process.pid, "SIGINT"); } catch { return; }
  await Bun.sleep(800);
  if (process.exitCode === null) try { globalThis.process.kill(-process.pid, "SIGKILL"); } catch { /* exited */ }
}
