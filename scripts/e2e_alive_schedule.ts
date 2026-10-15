import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = String(49820 + Math.floor(Math.random() * 200));
const dataDir = mkdtempSync(join(tmpdir(), "fugue-alive-schedule-"));
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
  const space = await post<{ id: string }>("/api/spaces", { title: "Scheduled refresh bookkeeping", projection: "belief" });

  const unauthorized = await fetch(`${baseUrl}/api/spaces/${space.id}/refresh-schedule`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ cadenceSeconds: 2, enabled: true })
  });
  assert(unauthorized.status === 401, `refresh schedule mutation must require auth, got ${unauthorized.status}`);

  const enabled = await put<RefreshSchedule>(`/api/spaces/${space.id}/refresh-schedule`, {
    cadenceSeconds: 2,
    enabled: true
  });
  assert(enabled.spaceId === space.id && enabled.cadenceSeconds === 2 && enabled.enabled, "enabled schedule response must match the request");
  const dueDelay = (Date.parse(enabled.nextDueAt) - Date.now()) / 1000;
  assert(dueDelay > 0 && dueDelay <= 2.25, `next due must be computed one cadence from now, got ${dueDelay}s`);

  const initialDue = enabled.nextDueAt;
  const advancedDue = await waitForAdvance(space.id, initialDue);
  const advancedBy = (Date.parse(advancedDue) - Date.parse(initialDue)) / 1000;
  assert(advancedBy === 2, `worker must advance exactly one cadence step, got ${advancedBy}s`);

  const disabled = await put<RefreshSchedule>(`/api/spaces/${space.id}/refresh-schedule`, {
    cadenceSeconds: 2,
    enabled: false
  });
  assert(!disabled.enabled, "schedule disable must be reflected by the endpoint");
  const disabledDue = sqlite("select next_due_at from refresh_schedule where space_id = " + literal(space.id) + ";");
  await Bun.sleep(2_300);
  assert(sqlite("select next_due_at from refresh_schedule where space_id = " + literal(space.id) + ";") === disabledDue, "disabled schedule must not advance");
  assert(sqlite("select enabled from refresh_schedule where space_id = " + literal(space.id) + ";") === "0", "disabled schedule must remain disabled");

  console.log("alive_schedule_e2e=passed");
  console.log(`space_id=${space.id}`);
  console.log(`schedule_initial_due=${initialDue}`);
  console.log(`schedule_advanced_due=${advancedDue}`);
  console.log(`schedule_advanced_seconds=${advancedBy}`);
  console.log(`schedule_disabled_due=${disabledDue}`);
} finally {
  await printScenarioCostAtTeardown(baseUrl, "alive-schedule");
  await stopProcessGroup(server);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

type RefreshSchedule = {
  spaceId: string;
  cadenceSeconds: number;
  nextDueAt: string;
  enabled: boolean;
  createdAt: string;
};

async function waitForAdvance(spaceId: string, initialDue: string): Promise<string> {
  for (let index = 0; index < 120; index += 1) {
    const due = sqlite("select next_due_at from refresh_schedule where space_id = " + literal(spaceId) + ";");
    if (due !== initialDue) return due;
    await Bun.sleep(50);
  }
  throw new Error(`schedule did not advance:\n${serverOutput}`);
}

async function registerUser(): Promise<string> {
  const response = await fetch(`${baseUrl}/api/auth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username: "alive-schedule-user", password: "correct horse battery staple" })
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

async function post<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: authCookie },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function put<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "PUT",
    headers: { "content-type": "application/json", cookie: authCookie },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

function sqlite(sql: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, "operations.sqlite"), ".timeout 5000", sql], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`sqlite3 operations.sqlite: ${result.stderr}`);
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
