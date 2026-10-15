import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = "49401";
const dataDir = mkdtempSync(join(tmpdir(), "fugue-worker-"));
const baseUrl = `http://127.0.0.1:${port}`;

const env = {
  ...process.env,
  FUGUE_API_PORT: port,
  FUGUE_DATA_DIR: dataDir
};

const init = spawnSync("./fugue", ["init"], {
  env,
  encoding: "utf8"
});
if (init.status !== 0) {
  throw new Error(`init failed:\n${init.stdout}\n${init.stderr}`);
}

sqlite(
  "operations.sqlite",
  "insert into worker_job (id, kind, state, payload_json, created_at, updated_at, claimed_at) values ('job-e2e-stale-running', 'unsupported_e2e', 'running', '{}', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));"
);
sqlite(
  "operations.sqlite",
  "insert into worker_attempt (id, job_id, state, started_at) values ('attempt-e2e-stale-running', 'job-e2e-stale-running', 'running', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));"
);

const apiServer = spawn("./fugue", ["api"], {
  env: {
    ...env
  },
  detached: true,
  stdio: ["ignore", "pipe", "pipe"]
});

let apiServerOutput = "";
apiServer.stdout.on("data", (chunk) => {
  apiServerOutput += chunk.toString();
});
apiServer.stderr.on("data", (chunk) => {
  apiServerOutput += chunk.toString();
});

try {
  await waitForHealth();
  await waitForWorkerFailure("job-e2e-stale-running");

  const staleJobState = sqlite("operations.sqlite", "select state from worker_job where id = 'job-e2e-stale-running';");
  const staleAttemptCount = sqlite("operations.sqlite", "select count(*) from worker_attempt where job_id = 'job-e2e-stale-running';");
  const staleOldAttemptState = sqlite("operations.sqlite", "select state from worker_attempt where id = 'attempt-e2e-stale-running';");
  const staleFailedAttemptCount = sqlite("operations.sqlite", "select count(*) from worker_attempt where job_id = 'job-e2e-stale-running' and id != 'attempt-e2e-stale-running' and state = 'failed';");
  const staleAttemptError = sqlite("operations.sqlite", "select error from worker_attempt where job_id = 'job-e2e-stale-running' and id != 'attempt-e2e-stale-running';");

  assert(staleJobState === "failed", `stale running worker job should be re-run and fail unsupported kind, got ${staleJobState}`);
  assert(staleAttemptCount === "2", `stale running worker job should create one new attempt, got ${staleAttemptCount}`);
  assert(staleOldAttemptState === "failed", `startup recovery should terminalize the interrupted attempt, got ${staleOldAttemptState}`);
  assert(staleFailedAttemptCount === "1", `stale running worker job should have one new failed attempt, got ${staleFailedAttemptCount}`);
  assert(
    staleAttemptError.includes("unsupported worker job kind"),
    `stale running worker job should record unsupported-kind error on the new attempt, got ${staleAttemptError}`
  );

  sqlite(
    "operations.sqlite",
    "insert into worker_job (id, kind, state, payload_json, created_at, updated_at) values ('job-e2e-unsupported', 'unsupported_e2e', 'queued', '{}', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));"
  );
  await waitForWorkerFailure("job-e2e-unsupported");

  const jobState = sqlite("operations.sqlite", "select state from worker_job where id = 'job-e2e-unsupported';");
  const attemptCount = sqlite("operations.sqlite", "select count(*) from worker_attempt where job_id = 'job-e2e-unsupported';");
  const attemptState = sqlite("operations.sqlite", "select state from worker_attempt where job_id = 'job-e2e-unsupported';");
  const attemptError = sqlite("operations.sqlite", "select error from worker_attempt where job_id = 'job-e2e-unsupported';");

  assert(jobState === "failed", `worker job should fail unsupported kind, got ${jobState}`);
  assert(attemptCount === "1", `worker job should create exactly one attempt, got ${attemptCount}`);
  assert(attemptState === "failed", `worker attempt should be failed, got ${attemptState}`);
  assert(
    attemptError.includes("unsupported worker job kind"),
    `worker attempt should record unsupported-kind error, got ${attemptError}`
  );

  console.log("worker_e2e=passed");
  console.log(`data_dir=${dataDir}`);
  console.log(`stale_job_state=${staleJobState}`);
  console.log(`stale_attempt_count=${staleAttemptCount}`);
  console.log(`stale_old_attempt_state=${staleOldAttemptState}`);
  console.log(`stale_failed_attempt_count=${staleFailedAttemptCount}`);
  console.log(`stale_attempt_error=${staleAttemptError}`);
  console.log(`job_state=${jobState}`);
  console.log(`attempt_count=${attemptCount}`);
  console.log(`attempt_state=${attemptState}`);
  console.log(`attempt_error=${attemptError}`);
} finally {
  await printScenarioCostAtTeardown(baseUrl, "worker");
  await stopProcessGroup(apiServer);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") {
    rmSync(dataDir, { recursive: true, force: true });
  }
}

async function waitForHealth() {
  const startedAt = Date.now();
  while (Date.now() - startedAt < 180_000) {
    try {
      const response = await fetch(`${baseUrl}/api/health`);
      if (response.ok) return;
    } catch {
      // Server is still starting.
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }

  throw new Error(`API did not become ready. Server output:\n${apiServerOutput}`);
}

async function waitForWorkerFailure(jobId: string) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < 10_000) {
    const state = sqlite("operations.sqlite", `select state from worker_job where id = ${literal(jobId)};`);
    if (state === "failed") return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`worker job ${jobId} did not fail within timeout`);
}

function sqlite(file: string, sql: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], {
    encoding: "utf8"
  });
  if (result.status !== 0) {
    throw new Error(`sqlite3 ${file} failed: ${result.stderr}`);
  }
  return result.stdout.trim();
}

function literal(value: string): string {
  return `'${value.replaceAll("'", "''")}'`;
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
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
