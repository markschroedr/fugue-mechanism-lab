import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ImportedSource, ReviewItem, SpaceRef, SynthesisDoc } from "../frontend/src/lib/types";
import { NGO_POST_ID, NGO_THREAD_URL, SYNTHETIC_FALSIFIER_COMMENT_ID, SYNTHETIC_FALSIFIER_QUOTE, startForumMagnumFixture, writeFixtureConfig } from "./lib/forum_magnum_fixture";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

type InputReceipt = { artifactId: string; jobId: string };
type JobReceipt = { jobId: string };
type Report = { artifactId: string; version: number; content: string };

const live = process.env.FUGUE_LIVE_LESSWRONG === "1";
const threadUrl = process.env.FUGUE_EVAL_THREAD_URL ?? NGO_THREAD_URL;
const expectedPostId = new URL(threadUrl).pathname.split("/")[2] ?? NGO_POST_ID;
const port = String(49417 + Math.floor(Math.random() * 200));
const dataDir = mkdtempSync(join(tmpdir(), "fugue-lesswrong-"));
const baseUrl = `http://127.0.0.1:${port}`;
const fixture = live ? null : startForumMagnumFixture(Number(port) + 1000);
const configPath = join(dataDir, "fugue.toml");
if (fixture) writeFixtureConfig(configPath, fixture.endpoint);
let authCookie = "";
const env = { ...process.env, FUGUE_API_PORT: port, FUGUE_DATA_DIR: dataDir, ...(fixture ? { FUGUE_CONFIG: configPath } : {}) };
const key = env.FUGUE_LLM_PROVIDER === "openrouter" ? env.OPENROUTER_API_KEY : env.OPENAI_API_KEY;
if (!key) throw new Error("the selected provider's LLM API key is required");

let serverOutput = "";
const server = spawn("./fugue", ["api"], { env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
server.stdout.on("data", (chunk) => (serverOutput += chunk.toString()));
server.stderr.on("data", (chunk) => (serverOutput += chunk.toString()));

try {
  await waitForHealth();
  authCookie = await registerUser();
  const space = await post<SpaceRef>("/api/spaces", { title: "Selective Optimism: imported reactions", projection: "belief" });
  const submittedAt = Date.now();
  const receipt = await post<InputReceipt>(`/api/spaces/${space.id}/input`, { kind: "text", content: threadUrl });
  const submissionMs = Date.now() - submittedAt;
  assert(submissionMs < 2_000, `LessWrong submission blocked on network for ${submissionMs}ms`);
  assert(receipt.artifactId === `lw-post-${expectedPostId}`, `unexpected root artifact ${receipt.artifactId}`);
  assert(
    sqlite("operations.sqlite", `select kind from worker_job where id = ${literal(receipt.jobId)};`) === "lesswrong_thread_fetch",
    "submission should enqueue a fetch worker job"
  );

  const sources = await waitForSources(space.id, receipt.artifactId);
  assert(sources.length >= 4, `expected a real comment tree, got ${sources.length} sources`);
  assert(sources[0]?.sourceKind === "post" && sources.every((source) => source.authorName.length > 0), "sources should preserve post/comment authorship");
  const actorCount = Number(sqlite("core.sqlite", "select count(*) from actor where id like 'lw-actor-%';"));
  assert(actorCount >= 3, `expected harvested authors, got ${actorCount}`);

  const review = await waitForReview(receipt.artifactId);
  const proposed = JSON.parse(sqlite("operations.sqlite", `select json_extract(payload_json, '$.findings') from attention_item where id = ${literal(review.id)};`)) as Array<{
    action: "attach" | "new";
    text: string;
    sourceArtifactIds: string[];
    stances: Array<{ evidenceArtifactId: string; falsifiers: Array<{ quote: string; condition: string }> }>;
  }>;
  assert(proposed.length >= 1 && proposed.length <= 6, `landscape delta should be coarse (1-6), got ${proposed.length}`);
  assert(proposed.every((position) => position.action === "new"), "an empty standing landscape must produce only new positions");
  assert(proposed.every((position) => position.sourceArtifactIds.length > 0), "every proposed position should cite new source artifacts");
  const proposedFalsifier = proposed
    .flatMap((position) => position.stances)
    .flatMap((stance) => stance.falsifiers.map((falsifier) => ({ ...falsifier, evidenceArtifactId: stance.evidenceArtifactId })))
    .find((falsifier) => falsifier.quote === SYNTHETIC_FALSIFIER_QUOTE);
  if (!fixture) {
    console.log("synthetic_falsifier_assertion=skipped_for_external_live_thread");
  } else {
    assert(proposedFalsifier, "fixture harvest must extract the synthetic falsifier statement");
    assert(
      proposedFalsifier.evidenceArtifactId === `lw-comment-${SYNTHETIC_FALSIFIER_COMMENT_ID}`,
      `synthetic falsifier must cite its exact authored artifact, got ${proposedFalsifier.evidenceArtifactId}`
    );
  }

  const accepted = await post<SynthesisDoc>(`/api/review/${review.id}/accept`, {});
  const entities = accepted.nodes.filter((node) => node.kind === "entity" && node.role === "issue");
  assert(entities.length === proposed.length, "accept should integrate the complete reviewed position landscape");
  const provenanceCount = Number(sqlite("core.sqlite", "select count(*) from relation where kind = 'derived_from';"));
  const stanceCount = Number(sqlite("core.sqlite", "select count(*) from stance where actor_id like 'lw-actor-%';"));
  assert(provenanceCount >= entities.length, "accepted positions should retain specific source provenance");
  assert(stanceCount > 0, "clear thread positions should produce harvested actor stances");
  const falsifierCount = Number(sqlite("core.sqlite", "select count(*) from contract where kind = 'falsifier';"));
  if (fixture) assert(falsifierCount >= 1, "accepting the fixture harvest must persist its falsifier contract");
  const persistedFalsifier = sqlite("core.sqlite", `select terms || '|' || evidence_artifact_id from contract where kind = 'falsifier' and terms = ${literal(SYNTHETIC_FALSIFIER_QUOTE)};`);
  if (fixture) assert(persistedFalsifier === `${SYNTHETIC_FALSIFIER_QUOTE}|lw-comment-${SYNTHETIC_FALSIFIER_COMMENT_ID}`, `persisted falsifier lost exact evidence: ${persistedFalsifier}`);
  const falsifierGroups = await get<Array<{ positionId: string; falsifiers: Array<{ terms: string }> }>>(`/api/spaces/${space.id}/falsifiers`);
  if (fixture) assert(falsifierGroups.some((group) => group.falsifiers.some((falsifier) => falsifier.terms === SYNTHETIC_FALSIFIER_QUOTE)), "deterministic falsifier read model must expose the harvested quote");

  const synthesisJob = await post<JobReceipt>(`/api/spaces/${space.id}/synthesis`, { mode: "full" });
  const report = await waitForReport(space.id);
  assert(report.content.length > 300, "synthesis should produce a substantive State-of-X report");
  if (fixture) assert(!report.content.includes(SYNTHETIC_FALSIFIER_QUOTE), "report prose must not narrate falsifiers");

  const secondReceipt = await post<InputReceipt>(`/api/spaces/${space.id}/input`, { kind: "text", content: threadUrl });
  assert(secondReceipt.artifactId === receipt.artifactId && secondReceipt.jobId !== receipt.jobId, "completed fetch must permit a first-class refresh job");
  const refreshSummary = await waitForRefresh(secondReceipt.jobId);
  const secondSources = await get<ImportedSource[]>(`/api/spaces/${space.id}/sources`);
  assert(/no new material/.test(refreshSummary), `expected honest no-op refresh, got ${refreshSummary}`);
  assert(secondSources.length === sources.length, "refresh must not duplicate artifacts");
  assert(Number(sqlite("core.sqlite", "select count(*) from actor where id like 'lw-actor-%';")) === actorCount, "re-ingest must not duplicate actors");

  console.log("lesswrong_thread_e2e=passed");
  console.log(`model=${env.FUGUE_LLM_MODEL ?? "provider default"}`);
  console.log(`thread_url=${threadUrl}`);
  console.log(`forum_magnum=${live ? "live" : "fixture"}`);
  if (fixture) console.log(`fixture_requests=${JSON.stringify(fixture.requests)}`);
  console.log(`space_id=${space.id}`);
  console.log(`job_id=${receipt.jobId}`);
  console.log(`submission_ms=${submissionMs}`);
  console.log(`source_artifacts=${sources.length}`);
  console.log(`harvested_actors=${actorCount}`);
  console.log(`review_id=${review.id}`);
  console.log(`review_summary=${review.summary}`);
  console.log(`proposed_landscape=${JSON.stringify(proposed)}`);
  console.log(`accepted_positions=${entities.length}`);
  console.log(`provenance_relations=${provenanceCount}`);
  console.log(`harvested_stances=${stanceCount}`);
  console.log(`harvested_falsifiers=${falsifierCount}`);
  console.log(`synthetic_falsifier=${persistedFalsifier}`);
  console.log(`falsifier_groups=${JSON.stringify(falsifierGroups)}`);
  console.log(`refresh_job_id=${secondReceipt.jobId}`);
  console.log(`refresh_outcome=${refreshSummary}`);
  console.log(`synthesis_job_id=${synthesisJob.jobId}`);
  console.log(`report_artifact_id=${report.artifactId}`);
  console.log(`report_version=${report.version}`);
  console.log("report_tail=");
  console.log(report.content.slice(-1200));
} finally {
  fixture?.server.stop(true);
  await printScenarioCostAtTeardown(baseUrl, "lesswrong-thread-live");
  await stopProcessGroup(server);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
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
  const started = Date.now();
  while (Date.now() - started < 30_000) {
    try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch { /* starting */ }
    await Bun.sleep(200);
  }
  throw new Error(`API did not start:\n${serverOutput}`);
}

async function waitForSources(spaceId: string, artifactId: string): Promise<ImportedSource[]> {
  const started = Date.now();
  while (Date.now() - started < 60_000) {
    const sources = await get<ImportedSource[]>(`/api/spaces/${spaceId}/sources`);
    if (sources.some((source) => source.artifactId === artifactId)) return sources;
    const failure = sqlite("operations.sqlite", `select coalesce(error, '') from worker_job where kind = 'lesswrong_thread_fetch' and json_extract(payload_json, '$.artifactId') = ${literal(artifactId)};`);
    if (failure) throw new Error(`LessWrong fetch failed: ${failure}\n${serverOutput}`);
    await Bun.sleep(200);
  }
  throw new Error(`LessWrong sources timed out:\n${serverOutput}`);
}

async function waitForReview(artifactId: string): Promise<ReviewItem> {
  const started = Date.now();
  while (Date.now() - started < 360_000) {
    const review = (await get<ReviewItem[]>("/api/review")).find((item) => item.kind === "ingest" && item.sourceArtifactId === artifactId);
    if (review) return review;
    const failure = sqlite("operations.sqlite", `select coalesce(error, '') from worker_job where json_extract(payload_json, '$.artifactId') = ${literal(artifactId)};`);
    if (failure) throw new Error(`LessWrong ingest failed: ${failure}\n${serverOutput}`);
    await Bun.sleep(750);
  }
  throw new Error(`LessWrong review timed out:\n${serverOutput}`);
}

async function waitForRefresh(jobId: string): Promise<string> {
  const started = Date.now();
  while (Date.now() - started < 60_000) {
    const row = sqlite("operations.sqlite", `select state || '|' || coalesce((select result_summary from worker_attempt where job_id = worker_job.id order by started_at desc limit 1), error, '') from worker_job where id = ${literal(jobId)};`);
    if (row.startsWith("succeeded|")) return row.slice("succeeded|".length);
    if (row.startsWith("failed|")) throw new Error(`refresh failed: ${row}\n${serverOutput}`);
    await Bun.sleep(200);
  }
  throw new Error(`refresh timed out:\n${serverOutput}`);
}

async function waitForReport(spaceId: string): Promise<Report> {
  const started = Date.now();
  while (Date.now() - started < 360_000) {
    const report = await get<Report | null>(`/api/spaces/${spaceId}/report`);
    if (report) return report;
    const failure = sqlite("operations.sqlite", "select coalesce(group_concat(error, '\n'), '') from worker_job where kind = 'synthesize' and state = 'failed';");
    const pending = Number(sqlite("operations.sqlite", "select count(*) from worker_job where kind = 'synthesize' and state in ('queued','running');"));
    if (failure && pending === 0) throw new Error(`synthesis failed: ${failure}\n${serverOutput}`);
    await Bun.sleep(750);
  }
  throw new Error(`synthesis timed out:\n${serverOutput}`);
}

async function get<T>(path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, { headers: { cookie: authCookie } });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, { method: "POST", headers: { "content-type": "application/json", cookie: authCookie }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

function sqlite(file: string, sql: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`sqlite3 ${file}: ${result.stderr}`);
  return result.stdout.trim();
}
function literal(value: string) { return `'${value.replaceAll("'", "''")}'`; }
function assert(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
async function stopProcessGroup(process: ReturnType<typeof spawn>) {
  if (!process.pid || process.exitCode !== null) return;
  try { globalThis.process.kill(-process.pid, "SIGINT"); } catch { return; }
  const exited = await new Promise<boolean>((resolve) => {
    const timer = setTimeout(() => resolve(false), 2_000);
    process.once("exit", () => { clearTimeout(timer); resolve(true); });
  });
  if (!exited) try { globalThis.process.kill(-process.pid, "SIGKILL"); } catch { /* exited */ }
}
