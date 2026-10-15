import { randomBytes } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

type CostTotals = { llmCalls: number; inputTokens: number; outputTokens: number; costUsd: number };

const results: CostTotals[] = [];
try {
  results.push(await runCondition("obols", 49431));
  results.push(await runCondition("independent", 49432));
  console.log("compute_backing_e2e=passed");
} finally {
  const total = results.reduce(
    (sum, item) => ({
      llmCalls: sum.llmCalls + item.llmCalls,
      inputTokens: sum.inputTokens + item.inputTokens,
      outputTokens: sum.outputTokens + item.outputTokens,
      costUsd: sum.costUsd + item.costUsd,
    }),
    { llmCalls: 0, inputTokens: 0, outputTokens: 0, costUsd: 0 },
  );
  console.log(
    `scenario_cost scenario=compute_backing llm_calls=${total.llmCalls} input_tokens=${total.inputTokens} output_tokens=${total.outputTokens} cost_usd=${total.costUsd.toFixed(8)}`,
  );
}

async function runCondition(mode: "obols" | "independent", port: number): Promise<CostTotals> {
  const root = mkdtempSync(join(tmpdir(), `fugue-compute-${mode}-`));
  const dataDir = join(root, "data");
  const kitDir = join(root, "kits");
  const home = join(root, "home");
  const configPath = join(root, "fugue.toml");
  const baseUrl = `http://127.0.0.1:${port}`;
  const substrateKey = randomBytes(32).toString("hex");
  const fakeExecutor = resolve("scripts/fixtures/fake_robinson_executor.py");
  const sourceConfig = readFileSync("fugue.toml", "utf8")
    .replace('data_dir = "data"', `data_dir = "${dataDir}"`)
    .replace("port = 49321", `port = ${port}`)
    .replace("weekly_allowance = 100", "weekly_allowance = 1")
    .replace('compute_backing = "obols"', `compute_backing = "${mode}"`)
    .replace('guest_api_url = "http://192.168.64.1:49321"', `guest_api_url = "${baseUrl}"`)
    .replace('kit_dir = "data/kits"', `kit_dir = "${kitDir}"`)
    .replace('executor_binary = "pi"', `executor_binary = "${fakeExecutor}"`)
    .replace("active_hours_start = 8", "active_hours_start = 0")
    .replace("active_hours_end = 22", "active_hours_end = 24")
    .replace("poll_seconds = 0.2", "poll_seconds = 0.01")
    .replace("quota_probe = true", "quota_probe = false");
  writeFileSync(configPath, sourceConfig);

  let serverOutput = "";
  const server = spawn("./fugue", ["api"], {
    env: { ...process.env, FUGUE_CONFIG: configPath, FUGUE_SUBSTRATE_KEY: substrateKey },
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  server.stdout.on("data", (chunk) => { serverOutput += chunk.toString(); });
  server.stderr.on("data", (chunk) => { serverOutput += chunk.toString(); });

  try {
    await waitForHealth(baseUrl, () => serverOutput);
    const birth = spawnSync("./fugue", ["society-birth", `Compute ${mode}`], {
      env: { ...process.env, FUGUE_CONFIG: configPath, FUGUE_SUBSTRATE_KEY: substrateKey },
      encoding: "utf8",
    });
    assert(birth.status === 0, `${mode} birth failed: ${birth.stderr || birth.stdout}`);
    const address = `compute-${mode}`;
    const kit = join(kitDir, address);
    const key = readFileSync(join(kit, "agent.key"), "utf8").trim();
    const extensionLoad = spawnSync("pi", ["--offline", "--extension", join(kit, "sleep.ts"), "--list-models", "__compute_backing_no_match__"], {
      env: { ...process.env, ROBINSON_COMPUTE_BACKING: mode },
      encoding: "utf8",
    });
    assert(extensionLoad.status === 0, `${mode} sleep tool must load with its runtime backing: ${extensionLoad.stderr || extensionLoad.stdout}`);

    const context = await agentGet<{ computeBacking: string; budget: { balance: number } }>(baseUrl, key, "/api/society/wake-context");
    assert(context.computeBacking === mode, `${mode} wake context must expose the active backing`);
    assert(context.budget.balance === 1, `${mode} agent must start with one Obol`);

    const transfer = await agentPostResponse(baseUrl, key, "/api/bank/transfers", { recipientAddress: "treasury", amount: 1 });
    assert(transfer.ok, `${mode} zero-balance setup failed: ${transfer.status} ${await transfer.text()}`);

    const zeroContext = await agentGet<{ computeBacking: string; budget: { balance: number } }>(baseUrl, key, "/api/society/wake-context");
    assert(zeroContext.budget.balance === 0, `${mode} condition must reach zero balance`);
    const permit = await agentPost<{ token: string }>(baseUrl, key, "/api/society/wakes/permit", { wakeNumber: 1 });
    const start = await agentPostResponse(baseUrl, key, "/api/society/wakes/start", { wakeNumber: 1, permitToken: permit.token });
    if (mode === "obols") {
      assert(start.status === 400, `obols mode must reject server-side wake admission at zero balance, got ${start.status}`);
    } else {
      assert(start.ok, `independent mode must admit a wake at zero balance: ${start.status} ${await start.text()}`);
    }
    const release = await agentPostResponse(baseUrl, key, "/api/society/wakes/permit/release", { wakeNumber: 1, token: permit.token });
    assert(release.ok, `${mode} permit release failed: ${release.status} ${await release.text()}`);
    const preview = spawnSync("python3", [join(kit, "runner.py"), "--kit", kit, "--home", home, "--preview-wake"], { encoding: "utf8" });
    assert(preview.status === 0, `${mode} preview failed: ${preview.stderr || preview.stdout}`);

    const runner = spawnSync("python3", [join(kit, "runner.py"), "--kit", kit, "--home", home, "--max-wakes", "1"], {
      encoding: "utf8",
      timeout: 20_000,
    });
    assert(runner.status === 0, `${mode} runner failed: ${runner.stderr || runner.stdout}`);
    const session = join(home, "sessions", "session.jsonl");
    if (mode === "obols") {
      assert(!existsSync(session), "obols mode must halt admission at zero balance");
    } else {
      assert(existsSync(session), "independent mode must admit a wake at zero balance");
      const sessionHeader = JSON.parse(readFileSync(session, "utf8").split("\n", 1)[0]);
      assert(sessionHeader.computeBacking === "independent", "runner must export independent backing to the executor environment");
      const usage = await agentGet<Array<{ wakeNumber: number; inputTokens: number; outputTokens: number; costUsd: number }>>(baseUrl, key, `/api/bank/usage?agent=${address}`);
      assert(usage.length === 1 && usage[0].wakeNumber === 1 && usage[0].inputTokens === 1006 && usage[0].outputTokens === 2 && usage[0].costUsd > 0, "independent mode must retain token and USD observations");
      const debit = await agentPostResponse(baseUrl, key, "/api/society/wakes/debit");
      assert(debit.status === 400, `independent mode must reject the wake-debit endpoint, got ${debit.status}`);
      const after = await agentGet<{ budget: { balance: number } }>(baseUrl, key, "/api/society/wake-context");
      assert(after.budget.balance === 0, "independent wake must not debit Obols");
    }

    const costs = await publicGet<{ overall: CostTotals }>(baseUrl, "/api/costs");
    return costs.overall;
  } finally {
    await stopProcessGroup(server);
    if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(root, { recursive: true, force: true });
  }
}

async function waitForHealth(baseUrl: string, output: () => string) {
  const started = Date.now();
  while (Date.now() - started < 30_000) {
    try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch { /* starting */ }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`API did not become ready:\n${output()}`);
}

async function publicGet<T>(baseUrl: string, path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`);
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return await response.json() as T;
}

async function agentGet<T>(baseUrl: string, key: string, path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, { headers: { authorization: `Bearer ${key}` } });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return await response.json() as T;
}

function agentPostResponse(baseUrl: string, key: string, path: string, body?: unknown) {
  return fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${key}` },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
}

async function agentPost<T>(baseUrl: string, key: string, path: string, body: unknown): Promise<T> {
  const response = await agentPostResponse(baseUrl, key, path, body);
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return await response.json() as T;
}

async function stopProcessGroup(child: ReturnType<typeof spawn>) {
  if (child.exitCode !== null) return;
  try { process.kill(-child.pid!, "SIGTERM"); } catch { /* gone */ }
  await new Promise((resolve) => child.once("close", resolve));
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}
