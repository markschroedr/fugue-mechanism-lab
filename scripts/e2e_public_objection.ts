import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ReportVersion, ReviewItem, SpaceRef, SynthesisDoc, SynthesisReport } from "../frontend/src/lib/types";
import { NGO_THREAD_URL, startForumMagnumFixture, writeFixtureConfig } from "./lib/forum_magnum_fixture";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const live = process.env.FUGUE_LIVE_LESSWRONG === "1";
const threadUrl = process.env.FUGUE_EVAL_THREAD_URL ?? NGO_THREAD_URL;
const port = String(49820 + Math.floor(Math.random() * 200));
const dataDir = mkdtempSync(join(tmpdir(), "fugue-public-objection-"));
const baseUrl = `http://127.0.0.1:${port}`;
const fixture = live ? null : startForumMagnumFixture(Number(port) + 1000);
const configPath = join(dataDir, "fugue.toml");
if (fixture) writeFixtureConfig(configPath, fixture.endpoint);
const env = { ...process.env, FUGUE_API_PORT: port, FUGUE_DATA_DIR: dataDir, ...(fixture ? { FUGUE_CONFIG: configPath } : {}) };
const key = env.FUGUE_LLM_PROVIDER === "openrouter" ? env.OPENROUTER_API_KEY : env.OPENAI_API_KEY;
if (!key) throw new Error("the selected provider's LLM API key is required");

let operatorCookie = "";
let serverOutput = "";
const server = spawn("./fugue", ["api"], { env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
server.stdout.on("data", (chunk) => (serverOutput += chunk.toString()));
server.stderr.on("data", (chunk) => (serverOutput += chunk.toString()));

try {
  await waitForHealth();
  operatorCookie = await registerOperator();
  const space = await operatorPost<SpaceRef>("/api/spaces", {
    title: "Selective Optimism public objection proof",
    projection: "belief"
  });

  const importReceipt = await operatorPost<{ artifactId: string }>(`/api/spaces/${space.id}/input`, {
    kind: "text",
    content: threadUrl
  });
  const landscapeReview = await waitForReview((item) => item.kind === "ingest" && item.sourceArtifactId === importReceipt.artifactId);
  await operatorPost<SynthesisDoc>(`/api/review/${landscapeReview.id}/accept`, {});
  const baseline = await waitForReport(space.id, 1);
  assert(baseline.content.length > 300, "real Plan A synthesis should be substantive");

  const anonymousRead = await anonymousGet<SynthesisReport | null>(`/api/spaces/${space.id}/report`);
  assert(anonymousRead?.artifactId === baseline.artifactId, "anonymous reader should receive the current report");
  const quote = selectReportSpan(baseline.content);
  const objectionText = "This span omits a live objection: Plan A's centralized monitoring and enforcement could itself concentrate coercive power, so its control rationale and its governance risk point in opposite directions. Keep that unresolved tension explicit rather than presenting monitoring as one-sided reassurance.";
  const objection = await anonymousPost<ReviewItem>(`/api/spaces/${space.id}/objections`, {
    reportArtifactId: baseline.artifactId,
    reportVersion: baseline.version,
    start: quote.start,
    end: quote.end,
    quote: quote.text,
    objection: objectionText,
    displayName: "Plan A participant"
  });
  assert(objection.kind === "objection", `public objection should enter its own review presentation, got ${objection.kind}`);
  assert(objection.objectionAnchor?.quote === quote.text, "review response should display the immutable quoted span");
  assert(objection.objectionAnchor?.reportVersion === baseline.version, "review response should name the anchored report version");
  assert(objection.unverifiedDisplayName === "Plan A participant", "display name should be marked separately as unverified");

  const pending = await waitForReview((item) => item.id === objection.id);
  assert(pending.detail === objectionText, "operator review lane should show the objection text");
  assert(pending.objectionAnchor?.reportArtifactId === baseline.artifactId, "operator review should retain the original report artifact");
  const artifactShape = sqlite(
    "core.sqlite",
    `select kind || '|' || json_extract(metadata_json, '$.inputKind') || '|' || json_extract(metadata_json, '$.anchor.reportVersion') from artifact where id = ${literal(objection.sourceArtifactId!)};`
  );
  assert(artifactShape === `annotation|objection|${baseline.version}`, `objection should be a raw annotation artifact, got ${artifactShape}`);
  assert(
    sqlite("core.sqlite", `select kind || '|' || to_id from relation where from_id = ${literal(objection.sourceArtifactId!)};`) ===
      `annotates|${baseline.artifactId}`,
    "objection artifact should annotate the original report artifact"
  );

  await operatorPost<SynthesisDoc>(`/api/review/${objection.id}/accept`, {});
  const corrected = await waitForReport(space.id, baseline.version + 1);
  const history = await anonymousGet<ReportVersion[]>(`/api/spaces/${space.id}/report/history`);
  const correctedVersion = history.find((version) => version.version === corrected.version);
  assert(correctedVersion, "corrected report version should appear in anonymous history");
  const objectionCause = correctedVersion.causedBy.find((cause) => cause.kind === "objection_submitted");
  assert(objectionCause, "corrected report should name the accepted objection as a cause");
  assert(objectionCause.changeClassification, "accepted objection cause should carry an integration classification");
  assert(objectionCause.changeClassification !== "redundant", "the controlled real-content objection should require a visible correction");
  assert(corrected.content !== baseline.content, "a non-redundant accepted objection should change the report text");
  const correctionAdditions = correctedVersion.diffFromPrevious
    .filter((line) => line.kind === "added")
    .map((line) => line.text)
    .join("\n");
  assert(
    /monitor|enforc|coerc|governance risk|concentrat(?:e|ion).*power/i.test(correctionAdditions),
    `the visible correction should engage the anchored monitoring objection, added: ${correctionAdditions}`
  );
  assert(objectionCause.objectionAnchor?.quote === quote.text, "What changed should show the original quoted span");
  assert(objectionCause.objectionAnchor?.reportVersion === baseline.version, "What changed must not silently re-anchor to the new version");
  assert(objectionCause.unverifiedDisplayName === "Plan A participant", "What changed should keep the unverified display name explicit");

  const rejectionBaseCount = history.length;
  const rejectionQuote = selectReportSpan(corrected.content);
  const rejected = await anonymousPost<ReviewItem>(`/api/spaces/${space.id}/objections`, {
    reportArtifactId: corrected.artifactId,
    reportVersion: corrected.version,
    start: rejectionQuote.start,
    end: rejectionQuote.end,
    quote: rejectionQuote.text,
    objection: "Rejected-path proof: do not change the report."
  });
  await operatorPostEmpty(`/api/review/${rejected.id}/reject`);
  await Bun.sleep(2_000);
  const historyAfterRejection = await anonymousGet<ReportVersion[]>(`/api/spaces/${space.id}/report/history`);
  assert(historyAfterRejection.length === rejectionBaseCount, "rejecting an objection must not create a report version");

  const burstStatuses: number[] = [];
  for (let index = 0; index < 8; index += 1) {
    const response = await fetch(`${baseUrl}/api/spaces/${space.id}/objections`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        reportArtifactId: corrected.artifactId,
        reportVersion: corrected.version,
        start: rejectionQuote.start,
        end: rejectionQuote.end,
        quote: rejectionQuote.text,
        objection: `Rate-limit probe ${index}: this is deliberately distinct.`
      })
    });
    burstStatuses.push(response.status);
    if (response.status === 429) {
      const payload = (await response.json()) as { error?: string };
      assert(payload.error?.includes("rate limit"), "429 should explain that the objection rate limit was reached");
    }
  }
  assert(burstStatuses.includes(429), `anonymous objection burst should be rate limited, got ${burstStatuses.join(",")}`);

  const costs = await anonymousGet<{ servedModels: string[] }>("/api/costs");
  assert(costs.servedModels.length > 0, "live proof should record the provider-served model");
  console.log("public_objection_e2e=passed");
  console.log(`configured_model=${env.FUGUE_LLM_MODEL ?? "fugue.toml"}`);
  console.log(`served_models=${costs.servedModels.join(",")}`);
  console.log(`thread_url=${threadUrl}`);
  console.log(`forum_magnum=${live ? "live" : "fixture"}`);
  console.log(`space_id=${space.id}`);
  console.log(`baseline_report=v${baseline.version}:${baseline.artifactId}`);
  console.log(`objection_artifact_id=${objection.sourceArtifactId}`);
  console.log(`objection_review_id=${objection.id}`);
  console.log(`anchor=v${baseline.version}:${quote.start}-${quote.end}:${JSON.stringify(quote.text)}`);
  console.log(`corrected_report=v${corrected.version}:${corrected.artifactId}`);
  console.log(`correction_classification=${objectionCause.changeClassification}`);
  console.log(`correction_reason=${objectionCause.classificationReason}`);
  console.log(`history_versions=${history.map((version) => version.version).join(",")}`);
  console.log(`rejection_version_count=${historyAfterRejection.length}`);
  console.log(`rate_limit_statuses=${burstStatuses.join(",")}`);
  console.log("corrected_report_tail=");
  console.log(corrected.content.slice(-1400));
} finally {
  fixture?.server.stop(true);
  await printScenarioCostAtTeardown(baseUrl, "public-objection-live");
  await stopProcessGroup(server);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

function selectReportSpan(content: string): { text: string; start: number; end: number } {
  const candidates = content
    .split("\n")
    .map((candidate) => candidate.trim().replace(/^[-*]\s+/, ""))
    .filter((candidate) => !candidate.startsWith("#") && candidate.length >= 90);
  const line = candidates.find((candidate) => /\bmonitor(?:ing)?\b|\benforc(?:e|ement|ing)\b|\bcentraliz(?:e|ed|ation)\b|\bgovernance\b/i.test(candidate));
  assert(line, "real Plan A report should contain a substantive monitoring, enforcement, centralization, or governance span");
  const text = line.slice(0, Math.min(line.length, 220));
  const byteOffset = content.indexOf(text);
  assert(byteOffset >= 0, "selected reader span should occur verbatim in report source");
  const start = Array.from(content.slice(0, byteOffset)).length;
  return { text, start, end: start + Array.from(text).length };
}

async function registerOperator(): Promise<string> {
  const response = await fetch(`${baseUrl}/api/auth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username: "objection-operator", password: "correct horse battery staple" })
  });
  if (!response.ok) throw new Error(`operator registration failed: ${response.status} ${await response.text()}`);
  const cookie = response.headers.get("set-cookie")?.split(";", 1)[0];
  assert(cookie, "registration should set an operator session cookie");
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

async function waitForReview(predicate: (item: ReviewItem) => boolean): Promise<ReviewItem> {
  const started = Date.now();
  while (Date.now() - started < 360_000) {
    const items = await operatorGet<ReviewItem[]>("/api/review");
    const item = items.find(predicate);
    if (item) return item;
    const failure = sqlite("operations.sqlite", "select coalesce(group_concat(error, '\n'), '') from worker_job where state = 'failed';");
    if (failure) throw new Error(`worker failed: ${failure}\n${serverOutput}`);
    await Bun.sleep(700);
  }
  throw new Error(`review timed out:\n${serverOutput}`);
}

async function waitForReport(spaceId: string, minimumVersion: number): Promise<SynthesisReport> {
  const started = Date.now();
  while (Date.now() - started < 360_000) {
    const report = await anonymousGet<SynthesisReport | null>(`/api/spaces/${spaceId}/report`);
    if (report && report.version >= minimumVersion) return report;
    const failure = sqlite("operations.sqlite", "select coalesce(group_concat(error, '\n'), '') from worker_job where kind = 'synthesize' and state = 'failed';");
    if (failure) throw new Error(`synthesis failed: ${failure}\n${serverOutput}`);
    await Bun.sleep(700);
  }
  throw new Error(`report v${minimumVersion} timed out:\n${serverOutput}`);
}

async function anonymousGet<T>(path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`);
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function operatorGet<T>(path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, { headers: { cookie: operatorCookie } });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function anonymousPost<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function operatorPost<T = unknown>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: operatorCookie },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function operatorPostEmpty(path: string): Promise<void> {
  const response = await fetch(`${baseUrl}${path}`, { method: "POST", headers: { cookie: operatorCookie } });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
}

function sqlite(file: string, sql: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`sqlite3 ${file}: ${result.stderr}`);
  return result.stdout.trim();
}

function literal(value: string): string { return `'${value.replaceAll("'", "''")}'`; }
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
