import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const root = mkdtempSync(join(tmpdir(), "fugue-config-"));
const dataDir = join(root, "configured-data");
const configPath = join(root, "fugue.toml");
const apiPort = 49431;
const uiPort = 49432;
const previewPort = 49433;

writeFileSync(
  configPath,
  `[storage]
data_dir = ${JSON.stringify(dataDir)}

[api]
host = "127.0.0.1"
port = ${apiPort}

[ui]
host = "127.0.0.1"
port = ${uiPort}
preview_port = ${previewPort}

[llm]
model = "openai/gpt-5.6-luna"
effort = "medium"
request_timeout_seconds = 42

[llm.agents.full_generation]
effort = "high"

[sources.forum_magnum]
endpoint = "https://www.lesswrong.com/graphql"
`
);

const env = {
  ...process.env,
  FUGUE_CONFIG: configPath
};
for (const name of [
  "OPENAI_API_KEY",
  "OPENROUTER_API_KEY",
  "FUGUE_DATA_DIR",
  "FUGUE_API_HOST",
  "FUGUE_API_PORT",
  "FUGUE_UI_HOST",
  "FUGUE_UI_PORT",
  "FUGUE_PREVIEW_PORT",
  "FUGUE_LLM_MODEL",
  "VITE_FUGUE_API_URL"
]) {
  delete env[name];
}

const api = spawn("./fugue", ["api"], {
  env,
  detached: true,
  stdio: ["ignore", "pipe", "pipe"]
});
const ui = spawn("./fugue", ["ui"], {
  env,
  detached: true,
  stdio: ["ignore", "pipe", "pipe"]
});

let apiOutput = "";
let uiOutput = "";
api.stdout.on("data", (chunk) => (apiOutput += chunk.toString()));
api.stderr.on("data", (chunk) => (apiOutput += chunk.toString()));
ui.stdout.on("data", (chunk) => (uiOutput += chunk.toString()));
ui.stderr.on("data", (chunk) => (uiOutput += chunk.toString()));

try {
  await waitForUrl(`http://127.0.0.1:${apiPort}/api/health`, () => apiOutput);
  await waitForUrl(`http://127.0.0.1:${uiPort}`, () => uiOutput);

  const proxied = await fetch(`http://127.0.0.1:${uiPort}/api/health`);
  assert(proxied.ok, `UI proxy should reach configured API, got ${proxied.status}`);
  const health = (await proxied.json()) as { status?: string };
  assert(health.status === "ready", `proxied API health should be ready, got ${JSON.stringify(health)}`);

  for (const database of ["core.sqlite", "events.sqlite", "operations.sqlite"]) {
    assert(existsSync(join(dataDir, database)), `${database} should be created under configured data_dir`);
  }

  console.log("central_config_e2e=passed");
  console.log(`config_path=${configPath}`);
  console.log(`data_dir=${dataDir}`);
  console.log(`api_port=${apiPort}`);
  console.log(`ui_port=${uiPort}`);
  console.log(`preview_port=${previewPort}`);
} finally {
  await printScenarioCostAtTeardown(`http://127.0.0.1:${apiPort}`, "central-config");
  await Promise.all([stopProcessGroup(api), stopProcessGroup(ui)]);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") {
    rmSync(root, { recursive: true, force: true });
  }
}

async function waitForUrl(url: string, output: () => string) {
  const startedAt = Date.now();
  // A cold Rust build can take longer than 30 seconds on a clean checkout.
  while (Date.now() - startedAt < 120_000) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      // Process is still starting.
    }
    await Bun.sleep(200);
  }
  throw new Error(`${url} did not become ready.\n${output()}`);
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
