import { randomBytes } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const root = mkdtempSync(join(tmpdir(), "fugue-society-instances-"));
const gatePath = join(root, "wake-gate.sqlite");
const substrateKeys = [randomBytes(32).toString("hex"), randomBytes(32).toString("hex")];
const ports = [49441, 49442];
const outputs = ["", ""];
const servers: ReturnType<typeof spawn>[] = [];

const source = readFileSync("fugue.toml", "utf8");
try {
  for (let index = 0; index < 2; index += 1) {
    const instance = `experiment-${index + 1}`;
    const configPath = join(root, `${instance}.toml`);
    const dataDir = join(root, `${instance}-data`);
    const kitDir = join(root, `${instance}-kits`);
    const config = source
      .replace('data_dir = "data"', `data_dir = ${JSON.stringify(dataDir)}`)
      .replace("port = 49321", `port = ${ports[index]}`)
      .replace('guest_api_url = "http://192.168.64.1:49321"', `guest_api_url = "http://127.0.0.1:${ports[index]}"`)
      .replace('instance_id = "default"', `instance_id = ${JSON.stringify(instance)}`)
      .replace('kit_dir = "data/kits"', `kit_dir = ${JSON.stringify(kitDir)}`)
      .replace('wake_gate_path = "data/wake-gate.sqlite"', `wake_gate_path = ${JSON.stringify(gatePath)}`);
    writeFileSync(configPath, config);
    const server = spawn("./fugue", ["api"], {
      env: { ...process.env, FUGUE_CONFIG: configPath, FUGUE_SUBSTRATE_KEY: substrateKeys[index] },
      detached: true,
      stdio: ["ignore", "pipe", "pipe"]
    });
    server.stdout.on("data", (chunk) => { outputs[index] += chunk.toString(); });
    server.stderr.on("data", (chunk) => { outputs[index] += chunk.toString(); });
    servers.push(server);
    await waitForHealth(ports[index]!, () => outputs[index]!);
  }
  const first = birth(0, "Aster");
  const second = birth(0, "Bram");
  const third = birth(1, "Cleo");
  const fourth = birth(1, "Dara");

  const directoryA = await get<Array<{ address: string }>>(0, "/api/directory");
  const directoryB = await get<Array<{ address: string }>>(1, "/api/directory");
  assert(directoryA.map((entry) => entry.address).sort().join(",") === "aster,bram", "instance A must contain only its own agents");
  assert(directoryB.map((entry) => entry.address).sort().join(",") === "cleo,dara", "instance B must contain only its own agents");

  const permits = await Promise.all([
    permit(0, first.key, 1),
    permit(0, second.key, 1),
    permit(1, third.key, 1)
  ]);
  const duplicate = await fetch(`http://127.0.0.1:${ports[0]}/api/society/wakes/permit`, {
    method: "POST",
    headers: jsonHeaders(first.key),
    body: JSON.stringify({ wakeNumber: 1 })
  });
  const duplicateBody = await duplicate.text();
  assert(duplicate.status === 429 && !duplicateBody.includes(permits[0].token), "duplicate acquisition must not reveal or rotate the active permit token");
  const blocked = await fetch(`http://127.0.0.1:${ports[1]}/api/society/wakes/permit`, {
    method: "POST",
    headers: jsonHeaders(fourth.key),
    body: JSON.stringify({ wakeNumber: 1 })
  });
  assert(blocked.status === 429, `the fourth host-wide wake must wait, got ${blocked.status}`);

  await finish(0, first.key, 1, permits[0].token);
  const admitted = await permit(1, fourth.key, 1);
  assert(admitted.token.length > 10, "a released slot must admit the waiting wake");

  await substratePut(0, "/api/society/wake-line", { text: "Build one concrete thing today." });
  const contextA = await agentGet<{ sharedDailyLine: string }>(0, first.key, "/api/society/wake-context");
  const contextB = await agentGet<{ sharedDailyLine: string }>(1, third.key, "/api/society/wake-context");
  assert(contextA.sharedDailyLine === "Build one concrete thing today.", "the instance wake line must reach its own agents");
  assert(contextB.sharedDailyLine === "", "the instance wake line must not cross society boundaries");

  const usageA = await agentGet<unknown[]>(0, first.key, "/api/bank/usage?agent=aster");
  const usageB = await agentGet<unknown[]>(1, third.key, "/api/bank/usage?agent=cleo");
  assert(usageA.length === 0 && usageB.length === 0, "preparing instances and acquiring permits must not create wakes");

  await Promise.all([
    finish(0, second.key, 1, permits[1].token),
    finish(1, third.key, 1, permits[2].token),
    finish(1, fourth.key, 1, admitted.token)
  ]);

  console.log("society_instances_e2e=passed");
  console.log("instances=2,isolation=true,max_active_wakes=3,prepared_wakes=0");
} finally {
  await Promise.all(ports.map((port, index) => printScenarioCostAtTeardown(`http://127.0.0.1:${port}`, `society-instance-${index + 1}`)));
  await Promise.all(servers.map(stopProcessGroup));
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(root, { recursive: true, force: true });
}

function birth(index: number, name: string): { key: string } {
  const result = spawnSync("./fugue", ["society-birth", name], {
    env: {
      ...process.env,
      FUGUE_CONFIG: join(root, `experiment-${index + 1}.toml`),
      FUGUE_SUBSTRATE_KEY: substrateKeys[index]
    },
    encoding: "utf8"
  });
  assert(result.status === 0, `birth failed for ${name}: ${result.stderr || result.stdout}`);
  const address = name.toLowerCase();
  return { key: readFileSync(join(root, `experiment-${index + 1}-kits`, address, "agent.key"), "utf8").trim() };
}

async function permit(index: number, key: string, wakeNumber: number): Promise<{ token: string }> {
  const response = await fetch(`http://127.0.0.1:${ports[index]}/api/society/wakes/permit`, {
    method: "POST",
    headers: jsonHeaders(key),
    body: JSON.stringify({ wakeNumber })
  });
  if (!response.ok) throw new Error(`permit failed: ${response.status} ${await response.text()}`);
  return await response.json() as { token: string };
}

async function finish(index: number, key: string, wakeNumber: number, token: string): Promise<void> {
  const response = await fetch(`http://127.0.0.1:${ports[index]}/api/society/wakes/permit/release`, {
    method: "POST",
    headers: jsonHeaders(key),
    body: JSON.stringify({ wakeNumber, token })
  });
  if (response.status !== 204) throw new Error(`permit release failed: ${response.status} ${await response.text()}`);
}

async function substratePut(index: number, path: string, body: unknown): Promise<void> {
  const response = await fetch(`http://127.0.0.1:${ports[index]}${path}`, {
    method: "PUT",
    headers: jsonHeaders(substrateKeys[index]),
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
}

async function agentGet<T>(index: number, key: string, path: string): Promise<T> {
  const response = await fetch(`http://127.0.0.1:${ports[index]}${path}`, { headers: { authorization: `Bearer ${key}` } });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return await response.json() as T;
}

async function get<T>(index: number, path: string): Promise<T> {
  const response = await fetch(`http://127.0.0.1:${ports[index]}${path}`);
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return await response.json() as T;
}

function jsonHeaders(key: string): Record<string, string> {
  return { "content-type": "application/json", authorization: `Bearer ${key}` };
}

async function waitForHealth(port: number, output: () => string): Promise<void> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < 120_000) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/api/health`)).ok) return;
    } catch {
      // The server is still starting.
    }
    await Bun.sleep(200);
  }
  throw new Error(`API ${port} did not become ready.\n${output()}`);
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

async function stopProcessGroup(process: ReturnType<typeof spawn>): Promise<void> {
  if (!process.pid || process.exitCode !== null) return;
  try {
    globalThis.process.kill(-process.pid, "SIGINT");
  } catch {
    return;
  }
  await new Promise<void>((resolve) => process.once("exit", () => resolve()));
}
