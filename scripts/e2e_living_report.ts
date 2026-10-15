import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ReportVersion, ReviewItem, SpaceRef, SynthesisDoc, SynthesisReport } from "../frontend/src/lib/types";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = String(49620 + Math.floor(Math.random() * 200));
const dataDir = mkdtempSync(join(tmpdir(), "fugue-living-report-"));
const baseUrl = `http://127.0.0.1:${port}`;
const env = { ...process.env, FUGUE_API_PORT: port, FUGUE_DATA_DIR: dataDir };
let authCookie = "";
let serverOutput = "";
const server = spawn("./fugue", ["api"], { env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
server.stdout.on("data", (chunk) => (serverOutput += chunk.toString()));
server.stderr.on("data", (chunk) => (serverOutput += chunk.toString()));

try {
  await waitForHealth();
  authCookie = await registerUser();
  const space = await post<SpaceRef>("/api/spaces", { title: "Living report history proof", projection: "belief" });

  const firstPosition = await post<ReviewItem>(`/api/spaces/${space.id}/entities`, {
    role: "issue",
    text: "Technical evaluation capacity may bind before capital."
  });
  await post<SynthesisDoc>(`/api/review/${firstPosition.id}/accept`, {});
  const reportV1 = await post<SynthesisReport>(`/api/spaces/${space.id}/report`, {
    content: "# State of evaluation capacity\n\nTechnical evaluation capacity may bind before capital."
  });

  const quote = "may bind before capital";
  const objectionStart = Array.from(reportV1.content).join("").indexOf(quote);
  const objection = await anonymousPost<ReviewItem>(`/api/spaces/${space.id}/objections`, {
    reportArtifactId: reportV1.artifactId,
    reportVersion: reportV1.version,
    start: objectionStart,
    end: objectionStart + Array.from(quote).length,
    quote,
    objection: "This should distinguish staffing capacity from capital rather than treating evaluation capacity as one bundle.",
    displayName: "Careful reader"
  });
  await post<SynthesisDoc>(`/api/review/${objection.id}/accept`, {});
  const reportV2 = await post<SynthesisReport>(`/api/spaces/${space.id}/report`, {
    content: "# State of evaluation capacity\n\nTechnical evaluation staffing may bind before capital."
  });

  assert(reportV1.version === 1 && reportV2.version === 2, `expected report versions 1→2, got ${reportV1.version}→${reportV2.version}`);
  assert(reportV2.content !== reportV1.content, "v2 should persist the human-authored textual change");
  assert(
    sqlite("events.sqlite", "select count(*) from event where kind = 'report_written' and decision_state = 'decided';") === "2",
    "human-authored report versions should emit decided report_written events"
  );

  const history = await get<ReportVersion[]>(`/api/spaces/${space.id}/report/history`);
  assert(history.length === 2, `expected two report versions, got ${history.length}`);
  assert(history[1]!.diffFromPrevious.some((line) => line.kind === "added"), "v1→v2 should have a readable added line");
  assert(history[0]!.causedBy.some((cause) => cause.kind === "entity_create_requested"), "v1 should retain its accepted-position cause");
  assert(history[1]!.causedBy.some((cause) => cause.kind === "objection_submitted"), "v2 should retain its accepted objection cause");

  const diff = await get<ReportDiff>(`/api/spaces/${space.id}/report/versions/2/diff`);
  assert(diff.version === 2 && diff.previousVersion === 1, "version diff must identify its adjacent versions");
  assert(diff.hunks.length === 1, `targeted edit should produce one hunk, got ${diff.hunks.length}`);
  assert(diff.hunks[0]!.lines.some((line) => line.kind === "removed" && /evaluation capacity/.test(line.text)), "diff hunk must retain removed text");
  assert(diff.hunks[0]!.lines.some((line) => line.kind === "added" && /evaluation staffing/.test(line.text)), "diff hunk must retain added text");
  assert(diff.hunks[0]!.causedBy.some((cause) => cause.eventId === history[1]!.causedBy[0]!.eventId && cause.kind === "objection_submitted"), "each changed hunk must name its accepted cause window");

  const feed = await get<FeedEntry[]>(`/api/spaces/${space.id}/feed`);
  assert(feed.length >= 4, `feed should include accepted causes and report versions, got ${feed.length}`);
  assert(feed.every((entry, index) => index === 0 || entry.createdAt >= feed[index - 1]!.createdAt), "feed entries must be chronological");
  assert(feed.some((entry) => entry.kind === "version-written" && entry.reportVersion === 2), "feed must show written versions");
  assert(feed.some((entry) => entry.kind === "cause-accepted" && entry.cause?.kind === "entity_create_requested"), "feed must show accepted causes");
  assert(feed.some((entry) => entry.kind === "objection-resolved" && entry.objectionOutcome === "accepted"), "feed must show resolved objections");
  const cursorIndex = Math.floor(feed.length / 2);
  const cursor = feed[cursorIndex]!.eventId;
  const after = await get<FeedEntry[]>(`/api/spaces/${space.id}/feed?after=${encodeURIComponent(cursor)}`);
  assert(after.length === feed.length - cursorIndex - 1, "after cursor must return only later feed entries");
  assert(after.every((entry) => !feed.slice(0, cursorIndex + 1).some((prior) => prior.eventId === entry.eventId)), "after cursor must not repeat prior entries");

  const delayed = await post<ReviewItem>(`/api/spaces/${space.id}/entities`, {
    role: "issue", text: "A pending proposal is not yet an integrated cause."
  });
  await post<SynthesisReport>(`/api/spaces/${space.id}/report`, { content: reportV2.content });
  await post<SynthesisDoc>(`/api/review/${delayed.id}/accept`, {});
  await post<SynthesisReport>(`/api/spaces/${space.id}/report`, { content: reportV2.content });
  await post<SynthesisReport>(`/api/spaces/${space.id}/report`, { content: reportV2.content });
  const delayedHistory = await get<ReportVersion[]>(`/api/spaces/${space.id}/report/history`);
  assert(delayedHistory[2]!.causedBy.length === 0, "pending proposals must not enter report causes");
  assert(delayedHistory[3]!.causedBy.length === 1 && delayedHistory[3]!.causedBy[0]!.kind === "entity_create_requested",
    "accepting a proposal older than the previous report must include it in the next cause window");
  assert(delayedHistory[4]!.causedBy.length === 0, "integrated causes must not replay in later reports");
  console.log("delayed_acceptance=unintegrated_then_included_once");

  const bulkSpace = await post<SpaceRef>("/api/spaces", { title: "Feed pagination proof", projection: "belief" });
  for (let version = 1; version <= 101; version += 1) {
    await post<SynthesisReport>(`/api/spaces/${bulkSpace.id}/report`, { content: `# Feed pagination\n\nVersion ${version}` });
  }
  const firstFeedPage = await get<FeedEntry[]>(`/api/spaces/${bulkSpace.id}/feed`);
  assert(firstFeedPage.length === 100, `feed page must be bounded at 100 entries, got ${firstFeedPage.length}`);
  const secondFeedPage = await get<FeedEntry[]>(`/api/spaces/${bulkSpace.id}/feed?after=${encodeURIComponent(firstFeedPage.at(-1)!.eventId)}`);
  assert(secondFeedPage.length === 1 && secondFeedPage[0]!.reportVersion === 101, "after cursor must expose entries beyond the first page");

  const anonymousReport = await fetch(`${baseUrl}/api/spaces/${space.id}/report`);
  const anonymousHistory = await fetch(`${baseUrl}/api/spaces/${space.id}/report/history`);
  const anonymousDoc = await fetch(`${baseUrl}/api/spaces/${space.id}/doc`);
  assert(anonymousReport.ok && anonymousHistory.ok && anonymousDoc.ok, "the shared UI read models must stay anonymous");
  const removedPublicRoute = await fetch(`${baseUrl}/public/spaces/${space.id}/report`);
  assert(removedPublicRoute.status === 404, "the rejected standalone public route must stay absent");

  console.log("living_report_e2e=passed");
  console.log(`space_id=${space.id}`);
  console.log(`report_v1_artifact=${reportV1.artifactId}`);
  console.log(`report_v2_artifact=${reportV2.artifactId}`);
  console.log(`history_versions=${history.map((version) => version.version).join("→")}`);
  console.log(`v1_caused_by=${JSON.stringify(history[0]!.causedBy)}`);
  console.log(`v2_caused_by=${JSON.stringify(history[1]!.causedBy)}`);
  console.log(`anonymous_read_statuses=${anonymousReport.status},${anonymousHistory.status},${anonymousDoc.status}`);
  console.log(`diff_hunks=${JSON.stringify(diff.hunks)}`);
  console.log(`feed_kinds=${feed.map((entry) => entry.kind).join(",")}`);
  console.log(`feed_after_cursor=${after.map((entry) => entry.eventId).join(",")}`);
  console.log(`feed_page_sizes=${firstFeedPage.length},${secondFeedPage.length}`);
  console.log(`removed_public_route_status=${removedPublicRoute.status}`);
} finally {
  await printScenarioCostAtTeardown(baseUrl, "living-report-history");
  await stopProcessGroup(server);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

async function registerUser(): Promise<string> {
  const response = await fetch(`${baseUrl}/api/auth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username: "living-report-user", password: "correct horse battery staple" })
  });
  if (!response.ok) throw new Error(`registration failed: ${response.status} ${await response.text()}`);
  const cookie = response.headers.get("set-cookie")?.split(";", 1)[0];
  if (!cookie) throw new Error("registration did not set a session cookie");
  return cookie;
}

async function waitForHealth() {
  for (let index = 0; index < 150; index += 1) {
    try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch { /* starting */ }
    await Bun.sleep(200);
  }
  throw new Error(`API did not start:\n${serverOutput}`);
}

async function get<T>(path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, { headers: { cookie: authCookie } });
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

async function post<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: authCookie },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

function sqlite(file: string, sql: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`sqlite3 ${file}: ${result.stderr}`);
  return result.stdout.trim();
}

type ReportCause = ReportVersion["causedBy"][number];
type DiffLine = ReportVersion["diffFromPrevious"][number];
type ReportDiff = { version: number; previousVersion: number | null; hunks: Array<{ lines: DiffLine[]; causedBy: ReportCause[] }> };
type FeedEntry = {
  eventId: string;
  kind: "version-written" | "cause-accepted" | "objection-resolved";
  label: string;
  createdAt: string;
  reportVersion?: number;
  cause?: ReportCause;
  objectionOutcome?: "accepted" | "rejected";
};

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
