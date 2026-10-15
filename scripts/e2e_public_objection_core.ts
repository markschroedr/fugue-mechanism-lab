import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ReviewItem, SpaceRef, SynthesisReport } from "../frontend/src/lib/types";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = String(49820 + Math.floor(Math.random() * 200));
const dataDir = mkdtempSync(join(tmpdir(), "fugue-public-objection-core-"));
const baseUrl = `http://127.0.0.1:${port}`;
const env = { ...process.env, FUGUE_API_PORT: port, FUGUE_DATA_DIR: dataDir };
let operatorCookie = "";
let serverOutput = "";
const server = spawn("./fugue", ["api"], { env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
server.stdout.on("data", (chunk) => (serverOutput += chunk.toString()));
server.stderr.on("data", (chunk) => (serverOutput += chunk.toString()));

try {
  await waitForHealth();
  operatorCookie = await registerOperator();
  const space = await operatorPost<SpaceRef>("/api/spaces", {
    title: "Public objection plumbing proof",
    projection: "belief"
  });
  const report = await operatorPost<SynthesisReport>(`/api/spaces/${space.id}/report`, {
    content: "# State of governance\n\nCentralized monitoring may improve coordination while also concentrating coercive power. This unresolved tension must remain visible."
  });
  const quote = selectReportSpan(report.content);
  const objectionText = "The report should distinguish monitoring capacity from legitimate authority.";
  const objection = await anonymousPost<ReviewItem>(`/api/spaces/${space.id}/objections`, {
    reportArtifactId: report.artifactId,
    reportVersion: report.version,
    start: quote.start,
    end: quote.end,
    quote: quote.text,
    objection: objectionText,
    displayName: "Concerned reader"
  });

  assert(objection.kind === "objection", `expected objection review kind, got ${objection.kind}`);
  assert(objection.objectionAnchor?.quote === quote.text, "review response should retain the exact report span");
  assert(objection.objectionAnchor?.reportVersion === report.version, "review response should retain the original report version");
  assert(objection.unverifiedDisplayName === "Concerned reader", "display name should remain explicitly unverified");
  const pending = (await operatorGet<ReviewItem[]>("/api/review")).find((item) => item.id === objection.id);
  assert(pending?.detail === objectionText, "operator review should show the objection text");

  const artifactShape = sqlite(
    "core.sqlite",
    `select kind || '|' || json_extract(metadata_json, '$.inputKind') || '|' || json_extract(metadata_json, '$.anchor.reportVersion') from artifact where id = ${literal(objection.sourceArtifactId!)};`
  );
  assert(artifactShape === `annotation|objection|${report.version}`, `unexpected objection artifact shape: ${artifactShape}`);
  assert(
    sqlite("core.sqlite", `select kind || '|' || to_id from relation where from_id = ${literal(objection.sourceArtifactId!)};`) === `annotates|${report.artifactId}`,
    "objection artifact should annotate the original report artifact"
  );

  await operatorPostEmpty(`/api/review/${objection.id}/reject`);
  assert((await operatorGet<ReviewItem[]>("/api/review")).every((item) => item.id !== objection.id), "rejected objection should leave the review lane");
  assert((await anonymousGet<unknown[]>(`/api/spaces/${space.id}/report/history`)).length === 1, "rejection must not create a report version");

  const absent = await fetch(`${baseUrl}/api/spaces/${space.id}/objections`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      reportArtifactId: report.artifactId,
      reportVersion: report.version,
      start: quote.start,
      end: quote.end,
      quote: "text absent from the report",
      objection: "This invalid anchor must be rejected."
    })
  });
  assert(absent.status === 400, `invalid quote should return 400, got ${absent.status}`);

  const burstStatuses: number[] = [];
  for (let index = 0; index < 6; index += 1) {
    const response = await fetch(`${baseUrl}/api/spaces/${space.id}/objections`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        reportArtifactId: report.artifactId,
        reportVersion: report.version,
        start: quote.start,
        end: quote.end,
        quote: quote.text,
        objection: `Rate-limit probe ${index}: deliberately distinct objection.`
      })
    });
    burstStatuses.push(response.status);
  }
  assert(burstStatuses.includes(429), `anonymous objection burst should be rate limited, got ${burstStatuses.join(",")}`);

  console.log("public_objection_core_e2e=passed");
  console.log(`space_id=${space.id}`);
  console.log(`baseline_report=v${report.version}:${report.artifactId}`);
  console.log(`objection_artifact_id=${objection.sourceArtifactId}`);
  console.log(`anchor=v${report.version}:${quote.start}-${quote.end}:${JSON.stringify(quote.text)}`);
  console.log(`rate_limit_statuses=${burstStatuses.join(",")}`);
} finally {
  await printScenarioCostAtTeardown(baseUrl, "public-objection-core");
  await stopProcessGroup(server);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

function selectReportSpan(content: string) {
  const text = "Centralized monitoring may improve coordination while also concentrating coercive power.";
  const byteOffset = content.indexOf(text);
  assert(byteOffset >= 0, "known report span should exist");
  const start = Array.from(content.slice(0, byteOffset)).length;
  return { text, start, end: start + Array.from(text).length };
}

async function registerOperator(): Promise<string> {
  const response = await fetch(`${baseUrl}/api/auth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username: "objection-core-operator", password: "correct horse battery staple" })
  });
  if (!response.ok) throw new Error(await response.text());
  return response.headers.get("set-cookie")!.split(";", 1)[0]!;
}
async function waitForHealth() { for (let index = 0; index < 150; index += 1) { try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch {} await Bun.sleep(200); } throw new Error(serverOutput); }
async function anonymousGet<T>(path: string): Promise<T> { const response = await fetch(baseUrl + path); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
async function operatorGet<T>(path: string): Promise<T> { const response = await fetch(baseUrl + path, { headers: { cookie: operatorCookie } }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
async function anonymousPost<T>(path: string, body: unknown): Promise<T> { const response = await fetch(baseUrl + path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
async function operatorPost<T>(path: string, body: unknown): Promise<T> { const response = await fetch(baseUrl + path, { method: "POST", headers: { "content-type": "application/json", cookie: operatorCookie }, body: JSON.stringify(body) }); if (!response.ok) throw new Error(await response.text()); return response.json() as Promise<T>; }
async function operatorPostEmpty(path: string): Promise<void> { const response = await fetch(baseUrl + path, { method: "POST", headers: { cookie: operatorCookie } }); if (!response.ok) throw new Error(await response.text()); }
function sqlite(file: string, sql: string) { const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" }); if (result.status !== 0) throw new Error(result.stderr); return result.stdout.trim(); }
function literal(value: string) { return `'${value.replaceAll("'", "''")}'`; }
function assert(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
async function stopProcessGroup(process: ReturnType<typeof spawn>) { if (!process.pid || process.exitCode !== null) return; try { globalThis.process.kill(-process.pid, "SIGINT"); } catch { return; } await Bun.sleep(500); if (process.exitCode === null) try { globalThis.process.kill(-process.pid, "SIGKILL"); } catch {} }
