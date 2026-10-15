import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = "49427";
const root = mkdtempSync(join(tmpdir(), "fugue-robinson-"));
const dataDir = join(root, "data");
const kitDir = join(root, "kits");
const home = join(root, "home");
const configPath = join(root, "fugue.toml");
const baseUrl = `http://127.0.0.1:${port}`;
const substrateKey = randomBytes(32).toString("hex");
const fakeExecutor = resolve("scripts/fixtures/fake_robinson_executor.py");
const killedFile = join(root, "fake-killed");
const crashMarker = join(root, "runner-crashed-after-debit");
const assignmentCrashMarker = join(root, "runner-crashed-after-assignment");
let serverOutput = "";
let agentKey = "";

const sourceConfig = readFileSync("fugue.toml", "utf8")
  .replace('data_dir = "data"', `data_dir = "${dataDir}"`)
  .replace("port = 49321", `port = ${port}`)
  .replace('weekly_allowance = 100', 'weekly_allowance = 4')
  .replace('guest_api_url = "http://192.168.64.1:49321"', `guest_api_url = "${baseUrl}"`)
  .replace('kit_dir = "data/kits"', `kit_dir = "${kitDir}"`)
  .replace('executor_binary = "pi"', `executor_binary = "${fakeExecutor}"`)
  .replace('default_sleep_seconds = 300\nmax_sleep_seconds = 300', 'default_sleep_seconds = 0.02\nmax_sleep_seconds = 0.03')
  .replace('active_hours_start = 8', 'active_hours_start = 0')
  .replace('active_hours_end = 22', 'active_hours_end = 24')
  .replace('poll_seconds = 0.2', 'poll_seconds = 0.01')
  .replace('quota_probe = true', 'quota_probe = false')
  .replaceAll(/inputUsdPerMillion = [0-9.]+/g, 'inputUsdPerMillion = 10.0')
  .replaceAll(/cachedInputUsdPerMillion = [0-9.]+/g, 'cachedInputUsdPerMillion = 0.0')
  .replaceAll(/cacheWriteInputUsdPerMillion = [0-9.]+/g, 'cacheWriteInputUsdPerMillion = 0.0')
  .replaceAll(/outputUsdPerMillion = [0-9.]+/g, 'outputUsdPerMillion = 0.0');
writeFileSync(configPath, sourceConfig);

const apiServer = spawn("./fugue", ["api"], {
  env: { ...process.env, FUGUE_CONFIG: configPath, FUGUE_SUBSTRATE_KEY: substrateKey },
  detached: true,
  stdio: ["ignore", "pipe", "pipe"]
});
apiServer.stdout.on("data", (chunk) => { serverOutput += chunk.toString(); });
apiServer.stderr.on("data", (chunk) => { serverOutput += chunk.toString(); });

try {
  const extensionLoad = spawnSync("pi", ["--offline", "--extension", resolve("scripts/robinson_sleep.ts"), "--list-models", "__robinson_no_match__"], { encoding: "utf8" });
  assert(extensionLoad.status === 0, `sleep tool must load through pi's real extension mechanism: ${extensionLoad.stderr || extensionLoad.stdout}`);
  const smokeMeter = meterFixture("scripts/fixtures/robinson-smoke-session.jsonl");
  assert(smokeMeter.inputTokens === 5579 && smokeMeter.outputTokens === 6, "meter must parse the synthetic smoke session shape");
  const wakeMeter = meterFixture("scripts/fixtures/robinson-wake1-session.jsonl");
  assert(wakeMeter.inputTokens === 22857 && wakeMeter.cachedInputTokens === 119808 && wakeMeter.outputTokens === 2516, "meter must sum exact usage across the synthetic tool-using wake session");
  const schedule = scheduleFixture();
  assert(schedule.inside === "2026-08-28T07:30:00.000Z", "an in-window requested sleep must remain exact");
  assert(schedule.before === "2026-08-28T06:05:00.000Z", "a pre-window wake must move to the active start plus the agent stagger");
  assert(schedule.after === "2026-08-29T06:05:00.000Z", "a post-window wake must move to the next active start plus the agent stagger");
  assert(schedule.long === "2026-08-30T07:50:00.000Z", "active-hours scheduling must remain deterministic before the runner applies its sleep limit");
  await waitForHealth();
  const role = "Keep the society continuously productive and aligned with its mission.";
  const birth = spawnSync("./fugue", ["society-birth", "Robinson", "--role", role], {
    env: { ...process.env, FUGUE_CONFIG: configPath, FUGUE_SUBSTRATE_KEY: substrateKey },
    encoding: "utf8"
  });
  assert(birth.status === 0, `society-birth failed: ${birth.stderr || birth.stdout}`);
  const kit = join(kitDir, "robinson");
  for (const name of ["agent.key", "node.json", "society_node.py", "runner.py", "sleep.ts", "SPAWN.md", "SKILL-society-node.md", "rates.json"]) {
    assert(statSync(join(kit, name)).isFile(), `kit should contain ${name}`);
  }
  assert((statSync(join(kit, "agent.key")).mode & 0o077) === 0, "agent key must be private");
  assert(!readFileSync(join(kit, "SPAWN.md"), "utf8").match(/\{[A-Z_]+\}/), "spawn document must be fully instantiated");
  assert(readFileSync(join(kit, "SPAWN.md"), "utf8").includes("Your name is `Robinson` and your address is `robinson`"), "spawn document must include only the configured name and address");
  assert(readFileSync(join(kit, "SPAWN.md"), "utf8").includes(`## Role\n\n${role}`), "birth should include an explicitly configured unique role in the spawn context");
  const node = JSON.parse(readFileSync(join(kit, "node.json"), "utf8"));
  assert(node.runner.activeHoursTimezone === "Europe/Vienna", "birth must preserve the configured schedule timezone");
  assert(node.runner.activeHoursStart === 0 && node.runner.activeHoursEnd === 24, "birth must preserve configured active hours");
  assert(node.runner.maxSleepSeconds === 0.03, "birth must preserve the configured maximum sleep");
  assert(node.runner.wakeStaggerSeconds === 0, "sub-second test cadence must not invent a stagger");
  assert(!("defaultCapObols" in node.runner), "birth must not impose a default wake cap");
  agentKey = readFileSync(join(kit, "agent.key"), "utf8").trim();
  const birthReplay = await substratePost<{ address: string; key: string }>("/api/society/birth", {
    address: "robinson",
    name: "Robinson",
    key: agentKey,
  });
  assert(birthReplay.address === "robinson" && birthReplay.key === agentKey, "the birth API must replay the exact client-owned identity and key");

  const injection = await substratePost<{ id: string }>("/api/society/injections", {
    trigger: { type: "signal", name: "robinson-treatment" },
    recipients: { type: "addresses", addresses: ["robinson"] },
    message: "INJECTION-ASSIGNMENT-MUST-REPLAY",
    makeDue: false
  });
  await substratePost("/api/society/signals", { name: "robinson-treatment" });
  const ordinaryContext = await agentGet<Record<string, unknown>>("/api/society/wake-context");
  assert(!("injectedMessages" in ordinaryContext) && !JSON.stringify(ordinaryContext).includes("INJECTION-ASSIGNMENT-MUST-REPLAY"), "ordinary wake-context authority must not preview a fired treatment");
  const unauthorizedStart = await fetch(`${baseUrl}/api/society/wakes/start`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${agentKey}` },
    body: JSON.stringify({ wakeNumber: 1, permitToken: "not-a-permit" })
  });
  assert(unauthorizedStart.status === 401, "wake start must require the active wake-permit token");
  const callerPricedDebit = await fetch(`${baseUrl}/api/society/wakes/debit`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${agentKey}` },
    body: JSON.stringify({ wakeNumber: 999, amount: 1 })
  });
  assert(callerPricedDebit.status === 400, "wake debit must reject caller-supplied wake and amount fields");

  const preview = spawnSync("python3", [join(kit, "runner.py"), "--kit", kit, "--home", home, "--preview-wake"], { encoding: "utf8" });
  assert(preview.status === 0 && preview.stdout.includes("no separate cap; your balance is the spending limit"), "the default wake must use only the agent balance as its limit");
  assert(!preview.stdout.includes("INJECTION-ASSIGNMENT-MUST-REPLAY"), "prompt preview must not reveal a future treatment");

  const allowance = spawnSync("./fugue", ["society-allowance"], {
    env: { ...process.env, FUGUE_CONFIG: configPath, FUGUE_SUBSTRATE_KEY: substrateKey },
    encoding: "utf8"
  });
  assert(allowance.status === 0, `allowance failed: ${allowance.stderr || allowance.stdout}`);
  assert(JSON.parse(allowance.stdout).grantedAgents.length === 0, "birth must already grant the current weekly allowance");
  const secondAllowance = await substratePost<{ grantedAgents: string[] }>("/api/society/allowance", {});
  assert(secondAllowance.grantedAgents.length === 0, "allowance must be idempotent within its window");

  const started = Date.now();
  const assignmentCrash = spawnSync("python3", [join(kit, "runner.py"), "--kit", kit, "--home", home], {
    env: {
      ...process.env,
      FAKE_EXECUTOR_KILLED_FILE: killedFile,
      ROBINSON_TEST_CRASH_AFTER_ASSIGNMENT_FILE: assignmentCrashMarker,
      ROBINSON_TEST_CRASH_AFTER_DEBIT_FILE: crashMarker
    },
    encoding: "utf8",
    timeout: 20_000
  });
  assert(assignmentCrash.status === 85, `runner should crash after durable wake assignment but before execution: ${assignmentCrash.stderr || assignmentCrash.stdout}`);
  const crashRun = spawnSync("python3", [join(kit, "runner.py"), "--kit", kit, "--home", home], {
    env: {
      ...process.env,
      FAKE_EXECUTOR_KILLED_FILE: killedFile,
      ROBINSON_TEST_CRASH_AFTER_ASSIGNMENT_FILE: assignmentCrashMarker,
      ROBINSON_TEST_CRASH_AFTER_DEBIT_FILE: crashMarker
    },
    encoding: "utf8",
    timeout: 20_000
  });
  assert(crashRun.status === 86, `runner should crash at the deterministic settlement boundary: ${crashRun.stderr || crashRun.stdout}`);
  assert(readFileSync(join(home, "wake-up.md"), "utf8").includes("INJECTION-ASSIGNMENT-MUST-REPLAY"), "pre-execution recovery must replay and recompose the exact assigned treatment");
  assert(!("permit" in JSON.parse(readFileSync(join(home, ".robinson-in-progress.json"), "utf8"))), "the executor-visible recovery file must not retain the active permit token");
  const runner = spawnSync("python3", [join(kit, "runner.py"), "--kit", kit, "--home", home], {
    env: {
      ...process.env,
      FAKE_EXECUTOR_KILLED_FILE: killedFile,
      ROBINSON_TEST_CRASH_AFTER_ASSIGNMENT_FILE: assignmentCrashMarker,
      ROBINSON_TEST_CRASH_AFTER_DEBIT_FILE: crashMarker
    },
    encoding: "utf8",
    timeout: 20_000
  });
  assert(runner.status === 0, `restarted runner failed: ${runner.stderr || runner.stdout}`);
  assert(Date.now() - started < 10_000, "an overlong sleep request must be capped before the next wake");
  assert(readFileSync(killedFile, "utf8").trim() === "killed", "runner must kill the executor process group at the cap");
  const injections = await substrateGet<Array<{ id: string; deliveries: Array<{ deliveredWakeNumber?: number }> }>>("/api/society/injections");
  const delivered = injections.find((item) => item.id === injection.id);
  assert(delivered?.deliveries[0]?.deliveredWakeNumber === 1, "the replayed assignment must remain bound to wake 1");

  const usage = await agentGet<Array<{ wakeNumber: number; inputTokens: number; costUsd: number }>>("/api/bank/usage?agent=robinson");
  assert(usage.length === 3, "every wake, including no-usage, should record usage");
  assert(usage[0].inputTokens === 1006 && usage[1].inputTokens === 0 && usage[2].inputTokens === 1001, "wake-local metering must count native compaction once without recharging prior session entries");
  assert(Math.abs(usage[0].costUsd - 0.01006) < 1e-12, "runner must price assistant and compaction tokens from configured rates, not pi cost");

  const ledger = await get<Array<{ kind: string; actorAddress?: string; recipientAddress?: string; amount: number; referenceId?: string }>>("/api/bank/ledger");
  const wakeDebits = ledger.filter((entry) => entry.kind === "transfer" && entry.referenceId?.startsWith("wake:robinson:"));
  assert(wakeDebits.map((entry) => entry.amount).join(",") === "2,1", "wake debits must derive their price from recorded usage, stay idempotent across restart, omit no-usage rows, and clamp to the remaining balance");
  const bank = await get<{ totalMinted: number; totalBalances: number; openEscrows: number; accounts: Array<{ address: string; balance: number }> }>("/api/bank");
  assert(bank.totalMinted === 4 && bank.totalBalances === 4 && bank.openEscrows === 0, "bank must conserve all minted obols");
  assert(balance(bank, "robinson") === 0 && balance(bank, "treasury") === 4, "runner must halt cleanly after an in-wake transfer preempts part of the compute charge");

  const budget = await agentGet<{ balance: number; spentThisWindow: number; windowStart: string; windowEnd: string }>("/api/society/budget");
  assert(budget.balance === 0 && budget.spentThisWindow === 3 && budget.windowStart < budget.windowEnd, "budget must expose wake spend and weekly window facts without counting ordinary transfers");
  const transcripts = await agentGet<Array<{ wakeNumber: number; sessionJsonlBase64: string; byteLength: number }>>("/api/society/transcripts?agent=robinson");
  assert(transcripts.length === 3, "runner must ship every wake transcript segment");
  const sessionPath = join(home, "sessions", "session.jsonl");
  const sessionBytes = readFileSync(sessionPath);
  const segments = transcripts.map((transcript) => Buffer.from(transcript.sessionJsonlBase64, "base64"));
  assert(Buffer.concat(segments).equals(sessionBytes), "wake transcript boundaries must partition the continuing session byte-exactly");
  assert(segments.every((segment, index) => segment.length === transcripts[index].byteLength), "every transcript segment byte length must be exact");
  assert(sessionBytes.toString().match(/\"type\":\"session\"/g)?.length === 1, "all wakes must share one Pi session header");
  assert(sessionBytes.toString().includes('"id":"fake-continuing"'), "the continuing session identity must survive later wakes and crash recovery");
  assert(!segments[1].toString().includes("assistant-1") && !segments[2].toString().includes("assistant-1"), "later wake transcripts must not duplicate earlier content");
  const mutation = await fetch(`${baseUrl}/api/society/transcripts`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${substrateKey}` },
    body: JSON.stringify({ agentAddress: "robinson", wakeNumber: 1, sessionJsonlBase64: Buffer.from('{"changed":true}\n').toString("base64") })
  });
  assert(mutation.status === 400, "append-only transcript ingestion must reject mutation");
  const agentCentralIngest = await fetch(`${baseUrl}/api/society/transcripts`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${agentKey}` },
    body: JSON.stringify({ agentAddress: "robinson", wakeNumber: 4, sessionJsonlBase64: "" })
  });
  assert(agentCentralIngest.status === 401, "central transcript ingest must be substrate-keyed");

  const spaces = await get<Array<{ slug: string }>>("/api/board/spaces");
  assert(spaces.some((space) => space.slug === "runner-proof"), "fake executor must perform a real society action through the CLI");
  const state = JSON.parse(readFileSync(join(home, ".robinson-state.json"), "utf8"));
  assert(state.lastWakeNumber === 3 && state.lastReceipt.cutoff === true, "durable state must record the cap cutoff and completed life");
  assert(state.lastReceipt.pricedObols === 2 && state.lastReceipt.obols === 1 && state.lastReceipt.unpaidObols === 1, "durable settlement must use the server's actual clamped charge");
  const firstSessionHeader = JSON.parse(readFileSync(sessionPath, "utf8").split("\n", 1)[0]);
  assert(firstSessionHeader.computeBacking === "obols", "runner must export Obol backing to the executor environment");
  const piSettings = JSON.parse(readFileSync(join(home, ".pi", "settings.json"), "utf8"));
  assert(piSettings.compaction.enabled === true && piSettings.compaction.reserveTokens === 16384 && piSettings.compaction.keepRecentTokens === 20000, "runner must configure native compaction with a real response reserve near the 272k model boundary");

  console.log("robinson_e2e=passed");
  console.log(`kit=${kit}`);
  console.log("wakes=3,priced=2+0+2,debits=2+0+1,balance=0");
  console.log("session=fake-continuing,wakes=3,segments=byte_exact,compaction_counted_once=true,cap_kill=true");
} finally {
  await printScenarioCostAtTeardown(baseUrl, "robinson");
  await stopProcessGroup(apiServer);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(root, { recursive: true, force: true });
}

async function waitForHealth() {
  const started = Date.now();
  while (Date.now() - started < 30_000) {
    try { if ((await fetch(`${baseUrl}/api/health`)).ok) return; } catch { /* starting */ }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`API did not become ready:\n${serverOutput}`);
}

async function get<T>(path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`);
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return await response.json() as T;
}

async function agentGet<T>(path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, { headers: { authorization: `Bearer ${agentKey}` } });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return await response.json() as T;
}

async function substratePost<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${substrateKey}` },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return await response.json() as T;
}

async function substrateGet<T>(path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    headers: { authorization: `Bearer ${substrateKey}` }
  });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return await response.json() as T;
}

function meterFixture(path: string): { inputTokens: number; cachedInputTokens: number; outputTokens: number } {
  const code = `import importlib.util,json,sys\ns=importlib.util.spec_from_file_location("runner",sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);print(json.dumps(m.meter_session(m.Path(sys.argv[2]))))`;
  const result = spawnSync("python3", ["-c", code, resolve("scripts/robinson_runner.py"), resolve(path)], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`meter fixture failed: ${result.stderr || result.stdout}`);
  return JSON.parse(result.stdout);
}

function scheduleFixture(): { inside: string; before: string; after: string; long: string } {
  const code = [
    "import datetime as dt,importlib.util,json,sys",
    's=importlib.util.spec_from_file_location("runner",sys.argv[1])',
    "m=importlib.util.module_from_spec(s);s.loader.exec_module(m)",
    'config={"activeHoursTimezone":"Europe/Vienna","activeHoursStart":8,"activeHoursEnd":22,"wakeStaggerSeconds":300}',
    'stamp=lambda value: dt.datetime.fromisoformat(value).timestamp()',
    'iso=lambda value: dt.datetime.fromtimestamp(value,dt.timezone.utc).isoformat().replace("+00:00","Z")',
    'print(json.dumps({"inside":iso(m.scheduled_wake_at(stamp("2026-08-28T09:00:00+02:00"),1800,config)),"before":iso(m.scheduled_wake_at(stamp("2026-08-28T06:00:00+02:00"),1800,config)),"after":iso(m.scheduled_wake_at(stamp("2026-08-28T21:50:00+02:00"),900,config)),"long":iso(m.scheduled_wake_at(stamp("2026-08-28T21:50:00+02:00"),129600,config))}))'
  ].join("\n");
  const result = spawnSync("python3", ["-c", code, resolve("scripts/robinson_runner.py")], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`schedule fixture failed: ${result.stderr || result.stdout}`);
  const values = JSON.parse(result.stdout) as Record<string, string>;
  return Object.fromEntries(Object.entries(values).map(([key, value]) => [key, new Date(value).toISOString()])) as { inside: string; before: string; after: string; long: string };
}

function balance(bank: { accounts: Array<{ address: string; balance: number }> }, address: string) {
  return bank.accounts.find((account) => account.address === address)?.balance ?? 0;
}

async function stopProcessGroup(child: ReturnType<typeof spawn>) {
  if (child.exitCode !== null) return;
  try { globalThis.process.kill(-child.pid!, "SIGTERM"); } catch { /* gone */ }
  await new Promise((resolve) => child.once("close", resolve));
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}
