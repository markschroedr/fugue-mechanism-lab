import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ReviewItem, SpaceRef, SynthesisDoc } from "../frontend/src/lib/types";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

type InputReceipt = {
  artifactId: string;
  jobId: string;
};

type WorkerJobReceipt = {
  jobId: string;
};

type SynthesisReport = {
  artifactId: string;
  spaceId: string;
  version: number;
  content: string;
  mode: "full" | "selective";
  createdAt: string;
};

const port = "49416";
const dataDir = mkdtempSync(join(tmpdir(), "fugue-ingest-synthesis-"));
const baseUrl = `http://127.0.0.1:${port}`;
let authCookie = "";
const env = {
  ...process.env,
  FUGUE_API_PORT: port,
  FUGUE_DATA_DIR: dataDir
};

const llmKey = env.FUGUE_LLM_PROVIDER === "openrouter" ? env.OPENROUTER_API_KEY : env.OPENAI_API_KEY;
if (!llmKey) {
  throw new Error("the selected provider's LLM API key is required for the real ingest/synthesis E2E scenario");
}

let apiServerOutput = "";
const apiServer = spawn("./fugue", ["api"], {
  env,
  detached: true,
  stdio: ["ignore", "pipe", "pipe"]
});
apiServer.stdout.on("data", (chunk) => (apiServerOutput += chunk.toString()));
apiServer.stderr.on("data", (chunk) => (apiServerOutput += chunk.toString()));

try {
  await waitForHealth();
  authCookie = await registerUser();

  const space = await post<SpaceRef>("/api/spaces", {
    title: "Demand-driven AI safety synthesis",
    projection: "belief"
  });
  const emptyDoc = await get<SynthesisDoc>(`/api/spaces/${space.id}/doc`);

  const documentReceipt = await post<InputReceipt>(`/api/spaces/${space.id}/input`, {
    kind: "document",
    content:
      "Capability evaluations are improving, but they still under-measure elicitation difficulty and post-deployment adaptation. The immediate synthesis should preserve this as one coarse issue rather than atomizing it."
  });
  assert(documentReceipt.artifactId.startsWith("artifact_"), "input should return its raw artifact id");
  assert(documentReceipt.jobId.startsWith("job_"), "input should return its ingest job id");

  const rawArtifact = sqlite(
    "core.sqlite",
    `select kind || '|' || media_type || '|' || content || '|' || json_extract(metadata_json, '$.spaceId') || '|' || json_extract(metadata_json, '$.inputKind') from artifact where id = ${literal(documentReceipt.artifactId)};`
  );
  assert(rawArtifact.startsWith("source|text/plain|Capability evaluations"), `document should be an append-only raw source artifact, got ${rawArtifact}`);
  assert(rawArtifact.endsWith(`|${space.id}|document`), `document metadata should carry space and input kind, got ${rawArtifact}`);
  assert(
    sqlite("operations.sqlite", `select kind || '|' || target_id from worker_job where id = ${literal(documentReceipt.jobId)};`) ===
      `ingest|${documentReceipt.artifactId}`,
    "submission should enqueue an ingest job targeting the raw artifact"
  );

  const ingestReview = await waitForIngestReview(documentReceipt.artifactId);
  assert(ingestReview.kind === "ingest", "ingest proposal should use the existing ingest review kind");
  assert(
    ingestReview.sourceArtifactId === documentReceipt.artifactId,
    "ingest proposal should identify the submitted source artifact"
  );
  const acceptedDoc = await post<SynthesisDoc>(`/api/review/${ingestReview.id}/accept`, {});
  assert(acceptedDoc.version === emptyDoc.version + 1, "accepting ingest should recompile and bump the node projection");
  const acceptedEntities = acceptedDoc.nodes.filter((node) => node.kind === "entity");
  assert(acceptedEntities.length === 1, `coarse ingest should propose one entity by default, got ${acceptedEntities.length}`);
  const acceptedEntity = acceptedEntities[0];
  assert(acceptedEntity?.kind === "entity", "accepted ingest should integrate an entity");
  assert(
    sqlite(
      "core.sqlite",
      `select kind || '|' || from_id || '|' || to_id from relation where from_id = ${literal(acceptedEntity.id)} and to_id = ${literal(documentReceipt.artifactId)};`
    ) === `derived_from|${acceptedEntity.id}|${documentReceipt.artifactId}`,
    "accepted entity should carry a derived_from provenance relation to the raw artifact"
  );

  const fullJob = await post<WorkerJobReceipt>(`/api/spaces/${space.id}/synthesis`, { mode: "full" });
  const fullReport = await waitForReport(space.id, 1);
  assert(fullJob.jobId.startsWith("job_"), "full synthesis should return its worker job id");
  assert(fullReport.mode === "full", "first report should record full mode");
  assert(fullReport.content.length > 120, "full synthesis should be substantive reader-facing prose");
  assert(
    sqlite("core.sqlite", `select kind from artifact where id = ${literal(fullReport.artifactId)};`) === "derived",
    "synthesis report should be a derived artifact"
  );

  const marker = "Selective edit verified without rewriting the report.";
  const selectiveJob = await post<WorkerJobReceipt>(`/api/spaces/${space.id}/synthesis`, {
    mode: "selective",
    instruction: `Use insert-after-anchor to append exactly this final paragraph after the report's final paragraph: ${marker}`
  });
  const selectiveReport = await waitForReport(space.id, 2);
  assert(selectiveJob.jobId.startsWith("job_"), "selective synthesis should return its worker job id");
  assert(selectiveReport.mode === "selective", "edited report should record selective mode");
  assert(selectiveReport.content.includes(marker), "selective edit should apply the requested insertion");
  assert(selectiveReport.content.includes(fullReport.content), "selective edit should preserve the prior report instead of rewriting it");
  assert(
    sqlite("core.sqlite", `select derived_from from artifact where id = ${literal(selectiveReport.artifactId)};`) === fullReport.artifactId,
    "selective report version should derive from the previous report artifact"
  );
  assert(
    sqlite("events.sqlite", `select count(*) from event where target_id in (${literal(fullReport.artifactId)}, ${literal(selectiveReport.artifactId)}) and kind = 'synthesis_report_written' and decision_state = 'decided';`) === "2",
    "each report version should have decided event history"
  );

  const commentReceipt = await post<InputReceipt>(`/api/spaces/${space.id}/input`, {
    kind: "comment",
    content: "This sentence needs a concrete source.",
    anchor: {
      artifactId: selectiveReport.artifactId,
      quote: marker,
      start: selectiveReport.content.indexOf(marker),
      end: selectiveReport.content.indexOf(marker) + marker.length
    }
  });
  assert(
    sqlite(
      "core.sqlite",
      `select kind || '|' || json_extract(metadata_json, '$.inputKind') || '|' || json_extract(metadata_json, '$.anchor.artifactId') from artifact where id = ${literal(commentReceipt.artifactId)};`
    ) === `annotation|comment|${selectiveReport.artifactId}`,
    "anchored comment should use the shared input path and persist span metadata on a raw annotation artifact"
  );
  assert(
    sqlite(
      "core.sqlite",
      `select kind || '|' || from_id || '|' || to_id from relation where from_id = ${literal(commentReceipt.artifactId)} and to_id = ${literal(selectiveReport.artifactId)};`
    ) === `annotates|${commentReceipt.artifactId}|${selectiveReport.artifactId}`,
    "anchored comment should relate the annotation artifact to the selected report artifact"
  );
  await waitForIngestReview(commentReceipt.artifactId);
  const commentContextTrace = apiServerOutput
    .split("\n")
    .find((line) => line.startsWith("context_composed agent=coarse_ingest") && line.includes("targetExcerpt"));
  assert(commentContextTrace, "comment ingest request should include the bounded target excerpt");

  const textReceipt = await post<InputReceipt>(`/api/spaces/${space.id}/input`, {
    kind: "text",
    content: "A free-form thought belongs on the same generalized path."
  });
  assert(
    sqlite("core.sqlite", `select json_extract(metadata_json, '$.inputKind') from artifact where id = ${literal(textReceipt.artifactId)};`) === "text",
    "arbitrary text should use the same input route with kind represented as data"
  );

  console.log("ingest_synthesis_e2e=passed");
  console.log(`data_dir=${dataDir}`);
  console.log(`space_id=${space.id}`);
  console.log(`source_artifact_id=${documentReceipt.artifactId}`);
  console.log(`ingest_review_id=${ingestReview.id}`);
  console.log(`accepted_entity_id=${acceptedEntity.id}`);
  console.log(`compiled_doc_version=${acceptedDoc.version}`);
  console.log(`full_report_version=${fullReport.version}`);
  console.log(`full_report_artifact_id=${fullReport.artifactId}`);
  console.log(`selective_report_version=${selectiveReport.version}`);
  console.log(`selective_report_artifact_id=${selectiveReport.artifactId}`);
  console.log(`selective_marker_present=${selectiveReport.content.includes(marker)}`);
  console.log(`comment_artifact_id=${commentReceipt.artifactId}`);
  console.log(`comment_context_trace=${commentContextTrace}`);
  console.log(`text_artifact_id=${textReceipt.artifactId}`);
} finally {
  await printScenarioCostAtTeardown(baseUrl, "ingest-synthesis-live");
  await stopProcessGroup(apiServer);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") {
    rmSync(dataDir, { recursive: true, force: true });
  }
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
  const startedAt = Date.now();
  while (Date.now() - startedAt < 20_000) {
    try {
      const response = await fetch(`${baseUrl}/api/health`);
      if (response.ok) return;
    } catch {
      // Server is still starting.
    }
    await Bun.sleep(200);
  }
  throw new Error(`API did not become ready. Server output:\n${apiServerOutput}`);
}

async function waitForIngestReview(sourceArtifactId: string): Promise<ReviewItem> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < 180_000) {
    const items = await get<ReviewItem[]>("/api/review");
    const item = items.find(
      (candidate) => candidate.kind === "ingest" && candidate.sourceArtifactId === sourceArtifactId
    );
    if (item) return item;
    const failed = sqlite("operations.sqlite", "select coalesce(group_concat(error, '\n'), '') from worker_job where kind = 'ingest' and state = 'failed';");
    if (failed) throw new Error(`ingest worker failed: ${failed}\n${apiServerOutput}`);
    await Bun.sleep(500);
  }
  throw new Error(
    `ingest proposal for source artifact ${sourceArtifactId} did not reach review. Server output:\n${apiServerOutput}`
  );
}

async function waitForReport(spaceId: string, version: number): Promise<SynthesisReport> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < 240_000) {
    const response = await fetch(`${baseUrl}/api/spaces/${encodeURIComponent(spaceId)}/report`);
    if (response.ok) {
      const report = (await response.json()) as SynthesisReport | null;
      if (report && report.version >= version) return report;
    }
    const failed = sqlite("operations.sqlite", "select coalesce(group_concat(error, '\n'), '') from worker_job where kind = 'synthesize' and state = 'failed';");
    if (failed) throw new Error(`synthesis worker failed: ${failed}\n${apiServerOutput}`);
    await Bun.sleep(500);
  }
  throw new Error(`report v${version} did not appear. Server output:\n${apiServerOutput}`);
}

async function get<T>(path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, { headers: { cookie: authCookie } });
  if (!response.ok) throw new Error(`${path} failed with ${response.status}: ${await response.text()}`);
  return (await response.json()) as T;
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: authCookie },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${path} failed with ${response.status}: ${await response.text()}`);
  return (await response.json()) as T;
}

function sqlite(file: string, sql: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`sqlite3 ${file} failed: ${result.stderr}`);
  return result.stdout.trim();
}

function literal(value: string): string {
  return `'${value.replaceAll("'", "''")}'`;
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

async function stopProcessGroup(process: ReturnType<typeof spawn>) {
  if (!process.pid || process.exitCode !== null) return;
  try {
    globalThis.process.kill(-process.pid, "SIGINT");
  } catch {
    return;
  }
  const exited = await new Promise<boolean>((resolve) => {
    const timer = setTimeout(() => resolve(false), 2_000);
    process.once("exit", () => {
      clearTimeout(timer);
      resolve(true);
    });
  });
  if (!exited) {
    try {
      globalThis.process.kill(-process.pid, "SIGKILL");
    } catch {
      // Process group already exited.
    }
  }
}
