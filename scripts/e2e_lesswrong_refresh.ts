import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { JSDOM } from "../frontend/node_modules/jsdom/lib/api.js";

import { renderPositionedMarkdown, resolveMarkdownRange } from "../frontend/src/lib/markdown-position";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const root = join(import.meta.dir, "..");
const dataDir = mkdtempSync(join(tmpdir(), "fugue-refresh-"));
const apiPort = 52000 + Math.floor(Math.random() * 1000);
const fixturePort = apiPort + 1000;
const baseUrl = `http://127.0.0.1:${apiPort}`;
const postId = "FixtureMerge123";
const threadUrl = `https://www.lesswrong.com/posts/${postId}/fixture-merge`;
let generation = 1;
let cookie = "";
let serverOutput = "";
const fixtureRequests: Array<{ postId: string; offset: number; limit: number }> = [];

const authors = {
  host: { _id: "fixture-host", displayName: "Fixture Host", slug: "fixture-host" },
  atlas: { _id: "atlas-author", displayName: "Atlas Author", slug: "atlas-author" },
  kepler: { _id: "kepler-author", displayName: "Kepler Author", slug: "kepler-author" },
};
const baseComments = [comment("c1", authors.atlas, "I am ~70% confident in the Atlas rehearsal position: quarterly rehearsals and explicit intervention checklists make rare interventions operationally feasible.")];
const refreshComments = [
  ...baseComments,
  comment("c2", authors.atlas, "I now hold the same Atlas rehearsal position with high confidence: quarterly drills and explicit intervention checklists preserve intervention competence across rotating teams."),
  comment("c3", authors.kepler, "I reject the Atlas position. The distinct Kepler position is that rehearsals cannot preserve tacit local adaptation, so scaling the plan makes intervention performance worse. Do not attach me to Atlas."),
];

const fixture = Bun.serve({
  port: fixturePort,
  async fetch(request) {
    const body = await request.json() as { variables: { postId: string; offset: number; limit: number } };
    fixtureRequests.push(body.variables);
    const comments = (generation === 1 ? baseComments : refreshComments).slice(body.variables.offset, body.variables.offset + body.variables.limit);
    return Response.json({ data: { post: { result: { _id: postId, title: "Fixture merge debate", slug: "fixture-merge", postedAt: "2026-07-01T00:00:00Z", contents: { plaintextMainText: "Which practices make rare interventions operationally feasible, and what fails when they scale?" }, user: authors.host } }, comments: { results: comments } } });
  },
});

const configPath = join(dataDir, "fugue.toml");
writeFileSync(configPath, `[llm]\nmodel = "openai/gpt-5.6-luna"\neffort = "medium"\n\n[sources.forum_magnum]\nendpoint = "http://127.0.0.1:${fixturePort}/graphql"\n`);
const env = { ...process.env, FUGUE_DATA_DIR: dataDir, FUGUE_API_PORT: String(apiPort), FUGUE_CONFIG: configPath, FUGUE_LLM_PROVIDER: "openrouter" };
if (!env.OPENROUTER_API_KEY) throw new Error("OPENROUTER_API_KEY is required");
const server = spawn("./fugue", ["api"], { cwd: root, env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
server.stdout.on("data", (chunk) => serverOutput += chunk.toString());
server.stderr.on("data", (chunk) => serverOutput += chunk.toString());

try {
  await waitForHealth();
  cookie = await register();
  const space = await post<{ id: string }>("/api/spaces", { title: "Fixture multi-source refresh", projection: "belief" });
  const first = await post<InputReceipt>(`/api/spaces/${space.id}/input`, { kind: "text", content: threadUrl });
  await waitForSourceCount(space.id, 2);
  const firstReview = await waitForReview(first.artifactId);
  const firstFindings = findings(firstReview.id);
  assert(firstFindings.every((finding) => finding.action === "new"), "empty landscape must route every finding to new");
  const atlasEntityId = firstFindings.find((finding) => /quarterly|rehearsal/i.test(finding.text))?.id;
  assert(atlasEntityId, "baseline did not record the Atlas standing position ID");
  const acceptedBaseline = await post<SynthesisDoc>(`/api/review/${firstReview.id}/accept`, {});
  const baselineAtlas = findHolder(acceptedBaseline, "Atlas Author");
  assert(baselineAtlas.evidence.length === 1 && baselineAtlas.evidence[0]?.artifactId === "lw-comment-c1", "Atlas stance must cite the exact authored comment, not the author's source bag");
  assert(baselineAtlas.evidence[0]?.authoredAt === "2026-07-02T00:00:00Z", "stance evidence must preserve the source's authored time");
  assert(baselineAtlas.acceptedAt !== baselineAtlas.evidence[0]?.authoredAt, "acceptance time must remain distinct from authored time");
  assert(baselineAtlas.statedConfidence === "~70%", "explicit numeric credence must be stored literally");
  assert(baselineAtlas.history.length === 0, "a first stance should have no history");
  const baseline = await waitForReport(space.id, 1);

  assert(baseline.provenanceAnchors.length > 0, "the report must expose at least one relation-backed position span");
  for (const anchor of baseline.provenanceAnchors) {
    const anchoredText = Array.from(baseline.content).slice(anchor.start, anchor.end).join("");
    const entityText = sqlite("core.sqlite", `select content from entity where id = ${literal(anchor.entityId)};`);
    assert(anchoredText === entityText, `provenance anchor ${anchor.id} must cover the accepted entity wording exactly`);
    const relatedSources = sqlite(
      "core.sqlite",
      `select coalesce(group_concat(to_id, ','), '') from (select to_id from relation where from_id = ${literal(anchor.entityId)} and kind = 'derived_from' order by to_id);`
    ).split(",").filter(Boolean);
    const exposedSources = anchor.sources.map((source) => source.artifactId).sort();
    assert(JSON.stringify(exposedSources) === JSON.stringify(relatedSources), `anchor ${anchor.id} sources must be the true entity relation chain`);
    assert(anchor.sources.every((source) => source.url?.startsWith("https://www.lesswrong.com/")), `anchor ${anchor.id} should expose original LessWrong links`);
  }

  const formatted = formattingSelectionFromRenderedDom(baseline.content);
  const formattedObjection = await anonymousPost<Review>(`/api/spaces/${space.id}/objections`, {
    reportArtifactId: baseline.artifactId,
    reportVersion: baseline.version,
    start: formatted.start,
    end: formatted.end,
    quote: formatted.rawQuote,
    objection: "Formatting-boundary proof: preserve this exact visible selection as a raw markdown anchor."
  });
  assert(formatted.rawQuote !== formatted.visibleQuote, "proof selection must cross rendered markdown formatting");
  const formattedPending = (await get<Review[]>("/api/review")).find((item) => item.id === formattedObjection.id);
  assert(formattedPending, "formatting-crossing objection must enter the real review lane");
  await postEmpty(`/api/review/${formattedObjection.id}/reject`);

  const absentResponse = await fetch(`${baseUrl}/api/spaces/${space.id}/objections`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      reportArtifactId: baseline.artifactId,
      reportVersion: baseline.version,
      start: formatted.start,
      end: formatted.end,
      quote: "text that is genuinely absent from the report",
      objection: "This request must be rejected."
    })
  });
  assert(absentResponse.status === 400, `absent report text must still be rejected, got ${absentResponse.status}`);

  generation = 2;
  const schedule = await put<RefreshSchedule>(`/api/spaces/${space.id}/refresh-schedule`, {
    cadenceSeconds: 1,
    enabled: true
  });
  assert(schedule.enabled && schedule.cadenceSeconds === 1, "refresh cadence must be enabled through the authenticated endpoint");
  await waitForSourceCount(space.id, 4);
  const disabledSchedule = await put<RefreshSchedule>(`/api/spaces/${space.id}/refresh-schedule`, {
    cadenceSeconds: 1,
    enabled: false
  });
  assert(!disabledSchedule.enabled, "live proof should disable the fixture cadence after it fires");
  const scheduledFetches = Number(sqlite("operations.sqlite", `select count(*) from worker_job where kind = 'lesswrong_thread_fetch' and json_extract(payload_json, '$.scheduled') = 1 and json_extract(payload_json, '$.spaceId') = ${literal(space.id)};`));
  assert(scheduledFetches >= 1, "worker cadence must enqueue the existing refresh job kind");
  const refreshReview = await waitForReview(first.artifactId, firstReview.id);
  const delta = findings(refreshReview.id);
  assert(delta.length > 0, "new fixture comments must produce a merge delta");
  const cited = new Set(delta.flatMap((finding) => finding.sourceArtifactIds));
  const c2Attachment = delta.find((finding) => finding.action === "attach" && finding.sourceArtifactIds.includes("lw-comment-c2"));
  const c3NewPosition = delta.find((finding) => finding.action === "new" && finding.sourceArtifactIds.includes("lw-comment-c3"));
  assert(c2Attachment?.targetEntityId === atlasEntityId, "same-position c2 evidence must attach to the recorded Atlas position");
  assert(c3NewPosition && !c3NewPosition.targetEntityId, "distinct c3 dissent must route to a new position");
  assert(cited.has("lw-comment-c2") && cited.has("lw-comment-c3"), "delta must process both newly inserted comments");
  assert(!cited.has("lw-comment-c1") && !cited.has(first.artifactId), "refresh delta must not cite old discourse");
  const acceptedRefresh = await post<SynthesisDoc>(`/api/review/${refreshReview.id}/accept`, {});
  const currentAtlas = findHolder(acceptedRefresh, "Atlas Author");
  assert(currentAtlas.evidence.length === 1 && currentAtlas.evidence[0]?.artifactId === "lw-comment-c2", "the current Atlas stance must cite only its newer evidencing comment");
  assert(currentAtlas.evidence[0]?.authoredAt === "2026-07-03T00:00:00Z", "current stance must expose newer authored time");
  assert(currentAtlas.statedConfidence === "high confidence", "qualitative confidence must remain literal rather than becoming a number");
  assert(currentAtlas.history.length === 1, "refresh must retain the superseded Atlas stance as history");
  assert(currentAtlas.history[0]?.evidence.length === 1 && currentAtlas.history[0]?.evidence[0]?.artifactId === "lw-comment-c1", "stance history must retain its exact prior evidence");
  assert(currentAtlas.history[0]?.evidence[0]?.authoredAt === "2026-07-02T00:00:00Z", "stance history must retain prior authored time");
  assert(currentAtlas.history[0]?.statedConfidence === "~70%", "stance history must retain prior literal credence");
  const keplerHolder = findHolder(acceptedRefresh, "Kepler Author");
  assert(keplerHolder.evidence.length === 1 && keplerHolder.evidence[0]?.artifactId === "lw-comment-c3", "Kepler stance must cite its exact comment");
  assert(keplerHolder.statedConfidence === undefined, "a stance without stated confidence must not gain inferred confidence");
  const updated = await waitForReport(space.id, baseline.version + 1);
  const causes = JSON.parse(sqlite("core.sqlite", `select metadata_json from artifact where id = ${literal(updated.artifactId)};`))?.causedBy ?? [];
  assert(causes.some((cause: { kind: string }) => cause.kind === "lesswrong_landscape_delta_accepted"), "accepted merge delta must become synthesis causes");
  assert(/Atlas|quarterly|drill/i.test(updated.content) && /Kepler|tacit local adaptation/i.test(updated.content), "reader-visible report must show the strengthened Atlas record and distinct Kepler dissent");
  const newArtifactCount = Number(sqlite("core.sqlite", "select count(*) from artifact where id in ('lw-comment-c2','lw-comment-c3');"));
  assert(newArtifactCount === 2, "refresh must insert exactly the two new comment artifacts");
  const costs = await get<{ servedModels: string[] }>("/api/costs");
  assert(costs.servedModels.length > 0, "fixture proof must record the provider-served synthesis model");

  console.log("lesswrong_refresh_fixture_e2e=passed");
  console.log(`configured_model=${env.FUGUE_LLM_MODEL ?? "openai/gpt-5.6-luna"}`);
  console.log(`served_models=${costs.servedModels.join(",")}`);
  console.log(`fixture_requests=${JSON.stringify(fixtureRequests)}`);
  console.log(`initial_findings=${JSON.stringify(firstFindings)}`);
  console.log(`refresh_delta=${JSON.stringify(delta)}`);
  console.log(`refresh_new_artifacts=${newArtifactCount}`);
  console.log(`scheduled_fetches=${scheduledFetches}`);
  console.log(`schedule_next_due=${schedule.nextDueAt}`);
  console.log(`synthesis_cause_kinds=${causes.map((cause: { kind: string }) => cause.kind).join(",")}`);
  console.log(`report_version=${updated.version}`);
  console.log(`formatting_anchor=${formatted.start}-${formatted.end}:${JSON.stringify(formatted.rawQuote)}`);
  console.log(`provenance_anchors=${JSON.stringify(baseline.provenanceAnchors)}`);
} finally {
  fixture.stop(true);
  await printScenarioCostAtTeardown(baseUrl, "lesswrong-refresh-live");
  await stopProcessGroup(server);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

type InputReceipt = { artifactId: string; jobId: string };
type RefreshSchedule = { spaceId: string; cadenceSeconds: number; nextDueAt: string; enabled: boolean; createdAt: string };
type Review = { id: string; sourceArtifactId?: string };
type SourceLink = { artifactId: string; label: string; url?: string; authoredAt?: string };
type HistoricalStance = { stance: string; acceptedAt: string; statedConfidence?: string; evidence: SourceLink[] };
type Holder = HistoricalStance & { actorId: string; actorName: string; history: HistoricalStance[] };
type SynthesisDoc = { nodes: Array<{ kind: string; holders?: Holder[] }> };
type ProvenanceAnchor = { id: string; start: number; end: number; entityId: string; sources: SourceLink[] };
type Finding = { id?: string; action: "attach" | "new"; targetEntityId?: string; text: string; reasoning: string; sourceArtifactIds: string[]; stances?: Array<{ actorId: string; value: string; evidenceArtifactId: string; statedConfidence?: string }> };
type Report = { artifactId: string; version: number; content: string; provenanceAnchors: ProvenanceAnchor[] };
function findHolder(doc: SynthesisDoc, actorName: string): Holder {
  const holder = doc.nodes.flatMap((node) => node.holders ?? []).find((candidate) => candidate.actorName === actorName);
  assert(holder, `accepted landscape must expose ${actorName}'s stance`);
  return holder;
}
function formattingSelectionFromRenderedDom(content: string) {
  const dom = new JSDOM("<!doctype html><body></body>");
  const previous = { document: globalThis.document, Node: globalThis.Node, NodeFilter: globalThis.NodeFilter };
  Object.assign(globalThis, { document: dom.window.document, Node: dom.window.Node, NodeFilter: dom.window.NodeFilter });
  try {
    const host = document.createElement("article");
    host.innerHTML = renderPositionedMarkdown(content, []);
    document.body.append(host);
    const runs = Array.from(host.querySelectorAll<HTMLElement>("[data-source-boundaries]"));
    const formattedIndex = runs.findIndex((element) => {
      const start = Number(element.dataset.sourceStart);
      const end = Number(element.dataset.sourceEnd);
      const raw = Array.from(content).slice(start, end).join("");
      return /\*\*[^*\n]+\*\*/.test(raw) || /\[[^\]\n]+\]\([^\n)]+\)/.test(raw);
    });
    assert(formattedIndex >= 0, "real synthesized report must contain mapped bold or link formatting");
    const firstRun = runs[formattedIndex]!;
    const lastRun = runs[formattedIndex + 1] ?? firstRun;
    const first = firstText(firstRun);
    const last = lastText(lastRun);
    assert(first && last, "formatted proof runs must contain selectable text");
    const range = document.createRange();
    range.setStart(first, 0);
    range.setEnd(last, last.data.length);
    const resolved = resolveMarkdownRange(host, range, content);
    const visibleQuote = range.toString().trim();
    assert(resolved.quote !== visibleQuote, "DOM selection must resolve across hidden markdown syntax");
    assert(/\*\*|\[[^\]]+\]\(/.test(resolved.quote), "resolved raw quote must retain its bold or link syntax");
    assert(Array.from(content).slice(resolved.start, resolved.end).join("") === resolved.quote, "DOM map must return the exact raw report span");
    return { ...resolved, rawQuote: resolved.quote, visibleQuote };
  } finally {
    Object.assign(globalThis, previous);
    dom.window.close();
  }
}
function firstText(root: Node): Text | null { const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT); return walker.nextNode() as Text | null; }
function lastText(root: Node): Text | null { const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT); let last: Text | null = null; for (let node = walker.nextNode(); node; node = walker.nextNode()) last = node as Text; return last; }
function comment(id: string, user: object, text: string) { return { _id: id, parentCommentId: null, postedAt: `2026-07-0${id === "c1" ? 2 : id === "c2" ? 3 : 4}T00:00:00Z`, contents: { plaintextMainText: text }, user }; }
function findings(reviewId: string): Finding[] { return JSON.parse(sqlite("operations.sqlite", `select json_extract(payload_json, '$.findings') from attention_item where id = ${literal(reviewId)};`)); }
async function waitForReview(artifactId: string, exclude?: string): Promise<Review> {
  for (let i = 0; i < 800; i++) {
    const review = (await get<Review[]>("/api/review")).find((item) => item.sourceArtifactId === artifactId && item.id !== exclude);
    if (review) return review;
    throwOnFailure(); await Bun.sleep(500);
  }
  throw new Error(`review timeout\n${serverOutput}`);
}
async function waitForSourceCount(spaceId: string, count: number) {
  for (let i = 0; i < 300; i++) {
    const sources = await get<Array<{ artifactId: string }>>(`/api/spaces/${spaceId}/sources`);
    if (sources.length === count) return;
    throwOnFailure(); await Bun.sleep(200);
  }
  throw new Error(`source timeout\n${serverOutput}`);
}
async function waitForReport(spaceId: string, version: number): Promise<Report> {
  for (let i = 0; i < 900; i++) {
    const report = await get<Report | null>(`/api/spaces/${spaceId}/report`);
    if (report && report.version >= version) return report;
    throwOnFailure(); await Bun.sleep(500);
  }
  throw new Error(`report timeout\n${serverOutput}`);
}
function throwOnFailure() { const errors = sqlite("operations.sqlite", "select coalesce(group_concat(error, '\\n'), '') from worker_job where state = 'failed';"); if (errors) throw new Error(`${errors}\n${serverOutput}`); }
async function waitForHealth() { for (let i = 0; i < 150; i++) { try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch {} await Bun.sleep(200); } throw new Error(serverOutput); }
async function register() { const response = await fetch(`${baseUrl}/api/auth/register`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ username: "refresh-e2e", password: "correct horse battery staple" }) }); assert(response.ok, await response.text()); return response.headers.get("set-cookie")!.split(";", 1)[0]!; }
async function get<T>(path: string): Promise<T> { const response = await fetch(baseUrl + path, { headers: { cookie } }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
async function post<T = unknown>(path: string, body: unknown): Promise<T> { const response = await fetch(baseUrl + path, { method: "POST", headers: { "content-type": "application/json", cookie }, body: JSON.stringify(body) }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
async function put<T = unknown>(path: string, body: unknown): Promise<T> { const response = await fetch(baseUrl + path, { method: "PUT", headers: { "content-type": "application/json", cookie }, body: JSON.stringify(body) }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
async function anonymousPost<T>(path: string, body: unknown): Promise<T> { const response = await fetch(baseUrl + path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
async function postEmpty(path: string): Promise<void> { const response = await fetch(baseUrl + path, { method: "POST", headers: { cookie } }); if (!response.ok) throw new Error(await response.text()); }
function sqlite(file: string, sql: string) { const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" }); if (result.status !== 0) throw new Error(result.stderr); return result.stdout.trim(); }
function literal(value: string) { return `'${value.replaceAll("'", "''")}'`; }
function assert(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
async function stopProcessGroup(process: ReturnType<typeof spawn>) { if (!process.pid || process.exitCode !== null) return; try { globalThis.process.kill(-process.pid, "SIGINT"); } catch { return; } await Bun.sleep(800); if (process.exitCode === null) try { globalThis.process.kill(-process.pid, "SIGKILL"); } catch {} }
