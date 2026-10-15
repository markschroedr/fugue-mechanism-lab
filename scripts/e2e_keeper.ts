import { spawn, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const root = mkdtempSync(join(tmpdir(), "fugue-keeper-"));
const config = join(root, "fugue.toml");
const port = 49439;
const baseUrl = `http://127.0.0.1:${port}`;
const key = randomBytes(32).toString("hex");
const sandbox = join(root, "sandbox");
const events = join(root, "events.jsonl");
mkdirSync(join(root, "state"));
writeFileSync(sandbox, `#!/usr/bin/env python3
import fcntl,json,os,signal,time
from pathlib import Path
root=Path(__file__).parent
signal.signal(signal.SIGTERM,signal.SIG_IGN)
with (root/'events.jsonl').open('a') as f:
 fcntl.flock(f,fcntl.LOCK_EX)
 f.write(json.dumps({'vm':__import__('sys').argv[2],'pid':os.getpid(),'time':time.time()})+'\\n');f.flush()
time.sleep(.35)
`);
chmodSync(sandbox, 0o755);
let source = readFileSync("fugue.toml", "utf8")
  .replace('data_dir = "data"', `data_dir = "${join(root, "data")}"`)
  .replace("port = 49321", `port = ${port}`)
  .replace('kit_dir = "data/kits"', `kit_dir = "${join(root, "kits")}"`)
  .replace('guest_api_url = "http://192.168.64.1:49321"', `guest_api_url = "${baseUrl}"`)
  .replace("default_sleep_seconds = 300", "default_sleep_seconds = 0")
  .replace("max_sleep_seconds = 300", "max_sleep_seconds = 0")
  .replace("active_hours_start = 8", "active_hours_start = 0")
  .replace("active_hours_end = 22", "active_hours_end = 24");
writeFileSync(config, source);
// This scenario proves admission fairness and cutoff, not host observation.
const hook = join(root, "admission-only-controller.py");
writeFileSync(hook, `class Controller:\n frozen=False\n def __init__(self,*args): pass\n def tick(self): return True\n def environment(self,*args): return {}\n def close(self): pass\n`);
const env = { ...process.env, FUGUE_CONFIG: config, FUGUE_SUBSTRATE_KEY: key, FUGUE_SANDBOX_BIN: sandbox, FUGUE_COHORT_HOOK: hook };
const server = spawn("./fugue", ["api"], { env, detached: true, stdio: "ignore" });
try {
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(baseUrl + "/api/health")).ok) break; } catch {}
    await Bun.sleep(50);
  }
  const names = ["Ada", "Emmy", "Sofia", "Carl", "Karen", "Leonhard", "Mary", "Srinivasa", "Terence"];
  for (const name of names) {
    const birth = spawnSync("./fugue", ["society-birth", name], { env, encoding: "utf8" });
    if (birth.status !== 0) throw new Error(birth.stderr || birth.stdout);
  }
  const cutoff = Date.now() + 3500;
  source = source.replace(/wall_timeout_seconds = \d+/, `$&\nhard_stop_at = "${new Date(cutoff).toISOString()}"`);
  writeFileSync(config, source);
  const keeper = spawn("./fugue", ["society-keeper"], { env, stdio: "ignore" });
  const active = new Set<number>();
  const seen = new Set<string>();
  let consumed = 0;
  let maximum = 0;
  let secondCycle = false;
  while (keeper.exitCode === null && Date.now() < cutoff + 3500) {
    for (const pid of active) { try { process.kill(pid, 0); } catch { active.delete(pid); } }
    let rows: {vm: string;pid: number;time: number}[] = [];
    try { rows = readFileSync(events, "utf8").trim().split("\n").filter(Boolean).map((line) => JSON.parse(line)); } catch {}
    for (const row of rows.slice(consumed)) {
      for (const pid of active) { try { process.kill(pid, 0); } catch { active.delete(pid); } }
      active.add(row.pid);
      maximum = Math.max(maximum, active.size);
      if (seen.has(row.vm)) { secondCycle = true; if (seen.size !== 9) throw new Error("admission starved a fresh participant"); }
      seen.add(row.vm);
    }
    consumed = rows.length;
    if (maximum > 3) throw new Error(`resident admission exceeded three: ${maximum}`);
    await Bun.sleep(20);
  }
  if (keeper.exitCode === null) { keeper.kill("SIGKILL"); throw new Error("keeper exceeded cutoff grace"); }
  if (keeper.exitCode !== 0 || maximum !== 3 || seen.size !== 9 || !secondCycle) throw new Error(`incomplete fair admission: ${keeper.exitCode}/${maximum}/${seen.size}`);
  for (const pid of active) { try { process.kill(pid, 0); throw new Error("executor survived cutoff"); } catch (error: any) { if (error.code !== "ESRCH") throw error; } }
  console.log(`keeper_e2e=passed maximum_resident=3 participants=9 fair_rotation=true cutoff=true artifact=${events}`);
} finally {
  try { await printScenarioCostAtTeardown(baseUrl, "keeper"); } finally {
    if (server.pid) process.kill(-server.pid, "SIGTERM");
  }
}
