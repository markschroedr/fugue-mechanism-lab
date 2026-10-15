import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = mkdtempSync(join(tmpdir(), "fugue-experiments-"));
const hostConfig = join(root, "host.toml");
const experiments = join(root, "experiments");
const secrets = join(root, "secrets");
const logs = join(root, "logs");
const gateways = join(root, "gateways");
const sandboxDir = join(root, "sandbox");
const sandboxPath = join(sandboxDir, "sandbox");
const sandboxLog = join(sandboxDir, "commands.log");
const cohortHook = join(root, "fixture-cohort.py");
writeFileSync(cohortHook, `import subprocess,os\ndef run_sandbox(host,package,arguments,capture):\n result=subprocess.run([host.sandbox,*arguments],check=True,text=True,capture_output=capture,env={**os.environ,**host.sandbox_env})\n return result.stdout if capture else ""\n`);
const vmBridgeHost = "192.168.64.1";
const controllerHost = "127.0.0.1";
mkdirSync(sandboxDir);
writeFileSync(
  sandboxPath,
  `#!/bin/sh
case "$1" in
  list) exit 0 ;;
  *) printf '%s\n' "$*" >> ${JSON.stringify(sandboxLog)} ;;
esac
`,
);
chmodSync(sandboxPath, 0o755);
writeFileSync(hostConfig, `experiments_dir = ${JSON.stringify(experiments)}\nsecrets_dir = ${JSON.stringify(secrets)}\nlogs_dir = ${JSON.stringify(logs)}\ngateways_dir = ${JSON.stringify(gateways)}\nsandbox_bin = ${JSON.stringify(sandboxPath)}\nserver_bin = "/definitely/missing/fugue-server"\nvm_bridge_host = ${JSON.stringify(vmBridgeHost)}\n`);
const env = {
  ...process.env,
  PYTHONPATH: resolve("scripts"),
  FUGUE_HOST_CONFIG: hostConfig,
  FUGUE_COHORT_HOOK: cohortHook,
  FUGUE_CONFIG: resolve("fugue.toml"),
  FUGUE_API_HOST: "0.0.0.0",
  FUGUE_API_PORT: "1",
  FUGUE_API_URL: "http://wrong-instance.invalid",
  FUGUE_SUBSTRATE_KEY: "wrong-instance-key",
};
let occupiedPort: ReturnType<typeof Bun.listen> | undefined;

const publicHostConfig = join(root, "public-host.toml");
writeFileSync(publicHostConfig, `experiments_dir = ${JSON.stringify(experiments)}\nsecrets_dir = ${JSON.stringify(secrets)}\nlogs_dir = ${JSON.stringify(logs)}\ngateways_dir = ${JSON.stringify(gateways)}\nvm_bridge_host = "8.8.8.8"\n`);
const unsafeHost = spawnSync("./fugue", ["experiment", "create", "unsafe-host"], {
  env: { ...env, FUGUE_HOST_CONFIG: publicHostConfig },
  encoding: "utf8",
});
assert(unsafeHost.status !== 0 && unsafeHost.stderr.includes("must be 127.0.0.1 or an RFC1918"), "host binding must reject public API addresses");
const reservedDefault = spawnSync("./fugue", ["experiment", "create", "default"], { env, encoding: "utf8" });
assert(reservedDefault.status !== 0 && reservedDefault.stderr.includes("is reserved"), "the unscoped default identity must not become an experiment package");
assert(!existsSync(join(experiments, "default")), "reserved identity rejection must not create a package");

try {
  occupiedPort = Bun.listen({
    hostname: controllerHost,
    port: 49325,
    socket: { data() {} },
  });
} catch {
  // Another local process already proves that this port is unavailable.
}

try {
  mkdirSync(join(secrets, "alpha"), { recursive: true });
  writeFileSync(join(secrets, "alpha", "substrate.key"), "stale-create-authority\n");
  const secretSymlinkTarget = join(root, "must-not-be-overwritten.key");
  writeFileSync(secretSymlinkTarget, "outside-authority\n");
  mkdirSync(join(secrets, "gamma"), { recursive: true });
  symlinkSync(secretSymlinkTarget, join(secrets, "gamma", "substrate.key"));
  await Promise.all([runAsync("create", "alpha"), runAsync("create", "gamma")]);
  const alpha = join(experiments, "alpha");
  const alphaConfig = readFileSync(join(alpha, "fugue.toml"), "utf8");
  const birthRetry = await exerciseBirthRetry(join(alpha, "fugue.toml"), join(root, "birth-retry-kits"));
  assert(birthRetry.firstFailedWithPending, "a crash after API commit must retain the deterministic pending kit");
  assert(birthRetry.requestKeys.length === 2 && birthRetry.requestKeys[0] === birthRetry.requestKeys[1], "birth retry must reuse the exact persisted client key");
  assert(birthRetry.finalKey === birthRetry.requestKeys[0] && !birthRetry.pendingExists, "birth retry must finalize the same committed key into the destination kit");
  assert(birthRetry.localIncompleteFinalized && birthRetry.localIncompleteKey.length === 64, "a pre-API pending directory without a key must rebuild and finalize safely");
  assert(birthRetry.failedPublicationLeftNoFinal, "a failed durable write must never publish a partial final key");
  assert(birthRetry.exclusiveWritePreserved, "durable first creation must never overwrite an existing key");
  const gammaConfig = readFileSync(join(experiments, "gamma", "fugue.toml"), "utf8");
  const alphaPort = sectionPort(alphaConfig, "api", "port");
  const gammaPort = sectionPort(gammaConfig, "api", "port");
  const alphaBindingPath = join(gateways, "alpha", "binding.json");
  const alphaBinding = JSON.parse(readFileSync(alphaBindingPath, "utf8"));
  assert(JSON.stringify(alphaBinding) === JSON.stringify({ instanceId: "alpha", apiHost: controllerHost, apiPort: alphaPort }), "create must write the exact host endpoint binding");
  assert(alphaPort !== 49325 && gammaPort !== 49325, "create must reject an API port already bound on the host");
  assert(alphaPort !== gammaPort, "concurrent creates must allocate distinct API ports");
  assert(alphaConfig.includes('instance_id = "alpha"'), "create must assign the package identity");
  assert(alphaConfig.includes(`host = "${controllerHost}"`), "create must keep the controller API on loopback");
  assert(alphaConfig.includes(`guest_api_url = "http://${vmBridgeHost}:${alphaPort}"`), "create must bind the guest URL exactly to the trusted VM bridge and API port");
  assert(alphaConfig.includes('data_dir = "data"'), "database paths must stay portable");
  assert(alphaConfig.includes('kit_dir = "agents/kits"'), "agent kits must stay inside the package");
  assert(alphaConfig.includes('sandbox_bin = ""'), "host runtime paths must stay outside the package");
  assert(alphaConfig.includes('compute_backing = "obols"'), "new experiment packages must inherit the typed baseline compute backing");
  const alphaSecret = readFileSync(join(secrets, "alpha", "substrate.key"), "utf8");
  assert(alphaSecret !== "stale-create-authority\n", "create must replace stale target authority with a fresh secret");
  assert(!lstatSync(join(secrets, "gamma", "substrate.key")).isSymbolicLink(), "create must replace a symlinked target secret with a regular secret");
  assert(readFileSync(secretSymlinkTarget, "utf8") === "outside-authority\n", "fresh authority creation must not overwrite a symlink target");
  const independentAlphaConfig = alphaConfig.replace('compute_backing = "obols"', 'compute_backing = "independent"');
  const configuredCompute = run("configure-compute", "alpha", "independent");
  assert(configuredCompute.includes("compute_backing=independent"), "configure-compute must report the selected typed backing");
  assert(readFileSync(join(alpha, "fugue.toml"), "utf8") === independentAlphaConfig, "configure-compute must change only society.compute_backing");
  const invalidCompute = spawnSync("./fugue", ["experiment", "configure-compute", "alpha", "shared"], { env, encoding: "utf8" });
  assert(invalidCompute.status !== 0 && invalidCompute.stderr.includes("invalid choice"), "configure-compute must reject compute modes outside its typed pair");
  assert(readFileSync(join(alpha, "fugue.toml"), "utf8") === independentAlphaConfig, "invalid compute configuration must not mutate the package");
  writeFileSync(join(alpha, "Caddyfile"), "host-specific\n");
  unlinkSync(join(alpha, ".paused"));
  const activeCompute = spawnSync("./fugue", ["experiment", "configure-compute", "alpha", "obols"], { env, encoding: "utf8" });
  assert(activeCompute.status !== 0 && activeCompute.stderr.includes("must be paused"), "configure-compute must reject an active package");
  assert(readFileSync(join(alpha, "fugue.toml"), "utf8") === independentAlphaConfig, "active package rejection must not mutate its config");
  const unsafePrepare = spawnSync("./fugue", ["experiment", "prepare", "alpha"], { env, encoding: "utf8" });
  assert(unsafePrepare.status !== 0 && unsafePrepare.stderr.includes("must be paused"), "prepare must reject an active package before it touches a VM");
  writeFileSync(join(alpha, ".paused"), "paused\n");
  run("configure-compute", "alpha", "obols");
  assert(readFileSync(join(alpha, "fugue.toml"), "utf8") === alphaConfig, "configure-compute must support restoring the paired baseline exactly");

  unlinkSync(alphaBindingPath);
  const importPrepare = spawnSync("./fugue", ["experiment", "prepare", "alpha"], { env, encoding: "utf8" });
  assert(importPrepare.status !== 0, "prepare fixture must stop at the deliberately unavailable server");
  assert(existsSync(alphaBindingPath), "prepare must import a paused package by creating its missing host binding before activation");
  assert(run("status", "alpha").includes("state=failed"), "failed prepare must expose explicit failed lifecycle state");
  const blockedRecovery = await exerciseRecovery("alpha", true);
  assert(blockedRecovery.state === "failed", "recovery must remain failed when VM quiescence cannot be proved");
  const prepareRecovery = await exerciseRecovery("alpha", false);
  assert(prepareRecovery.state === "paused", "successful recovery must return a failed prepare to paused");
  assert(JSON.stringify(prepareRecovery.serviceActions) === JSON.stringify(["stop", "uninstall", "verify-stopped"]), "recovery must stop and verify exact services before pausing");
  assert(prepareRecovery.quiescenceChecks === 2, "recovery must prove VM quiescence before capture and after service removal");
  const invalidRecovery = spawnSync("./fugue", ["experiment", "recover", "alpha"], { env, encoding: "utf8" });
  assert(invalidRecovery.status !== 0 && invalidRecovery.stderr.includes("requires failed state"), "the recovery CLI must reject a package that is already safely paused");

  const failedStart = spawnSync("./fugue", ["experiment", "start", "alpha"], { env, encoding: "utf8" });
  assert(failedStart.status !== 0, "start fixture must stop at the deliberately unavailable server");
  assert(run("status", "alpha").includes("state=failed"), "failed start must expose explicit failed lifecycle state");
  assert((await exerciseRecovery("alpha", false)).state === "paused", "failed start must use the supported recovery path");

  writeFileSync(join(alpha, "fugue.toml"), alphaConfig.replace('data_dir = "data"', 'data_dir = "../escaped-data"'));
  const escapedComputeConfig = readFileSync(join(alpha, "fugue.toml"), "utf8");
  const escapedCompute = spawnSync("./fugue", ["experiment", "configure-compute", "alpha", "independent"], { env, encoding: "utf8" });
  assert(escapedCompute.status !== 0 && escapedCompute.stderr.includes("storage.data_dir must equal"), "configure-compute must validate the complete package boundary before mutation");
  assert(readFileSync(join(alpha, "fugue.toml"), "utf8") === escapedComputeConfig, "package-boundary rejection must preserve the complete config");
  const escapedConfig = spawnSync("./fugue", ["experiment", "society", "alpha", "status"], { env, encoding: "utf8" });
  assert(escapedConfig.status !== 0 && escapedConfig.stderr.includes("storage.data_dir must equal"), "authority-bearing commands must reject package paths that resolve outside the package");
  assert(existsSync(join(alpha, ".paused")), "package-boundary rejection must happen before lifecycle mutation");
  writeFileSync(join(alpha, "fugue.toml"), alphaConfig);

  const boundaryTarget = join(root, "outside-package.txt");
  writeFileSync(boundaryTarget, "must stay outside\n");
  const boundaryDirectories = [join(alpha, "data"), join(alpha, "agents", "kits"), join(alpha, "agents", "homes"), join(alpha, "evidence")];
  for (const [index, directory] of boundaryDirectories.entries()) {
    const link = join(directory, `escape-${index}`);
    symlinkSync(boundaryTarget, link);
    const boundaryStatus = spawnSync("./fugue", ["experiment", "status", "alpha"], { env, encoding: "utf8" });
    assert(boundaryStatus.status !== 0 && boundaryStatus.stderr.includes("must not contain symlinks"), "data, kits, homes, and evidence must reject symlink escapes");
    unlinkSync(link);
  }
  const copyEscape = join(alpha, "evidence", "copy-escape");
  symlinkSync(boundaryTarget, copyEscape);
  const escapedCopy = spawnSync("./fugue", ["experiment", "copy", "alpha", "escaped-copy"], { env, encoding: "utf8" });
  assert(escapedCopy.status !== 0 && escapedCopy.stderr.includes("must not contain symlinks"), "copy must validate its complete source before traversal");
  assert(!existsSync(join(experiments, "escaped-copy")) && !existsSync(join(secrets, "escaped-copy")), "rejected copy sources must create no package or authority");
  unlinkSync(copyEscape);
  assert(readFileSync(boundaryTarget, "utf8") === "must stay outside\n", "package rejection must not mutate symlink targets");

  writeFileSync(
    join(alpha, "fugue.toml"),
    alphaConfig.replace(`guest_api_url = "http://${vmBridgeHost}:${alphaPort}"`, `guest_api_url = "http://${vmBridgeHost}:1"`),
  );
  const unsafeGuestUrl = spawnSync("./fugue", ["experiment", "society", "alpha", "status"], { env, encoding: "utf8" });
  assert(unsafeGuestUrl.status !== 0 && unsafeGuestUrl.stderr.includes("society.guest_api_url must equal"), "authority-bearing commands must reject a guest URL that differs from the trusted binding");
  writeFileSync(join(alpha, "fugue.toml"), alphaConfig);
  writeFileSync(alphaBindingPath, JSON.stringify({ ...alphaBinding, apiPort: 1 }) + "\n");
  const mismatchedSociety = spawnSync("./fugue", ["experiment", "society", "alpha", "status"], { env, encoding: "utf8" });
  assert(mismatchedSociety.status !== 0 && mismatchedSociety.stderr.includes("does not match package config"), "authority-bearing commands must reject a mismatched host binding");
  run("rebind", "alpha");
  assert(JSON.stringify(JSON.parse(readFileSync(alphaBindingPath, "utf8"))) === JSON.stringify(alphaBinding), "explicit rebind must atomically accept the paused package endpoint");

  const preparedLifecycle = await exercisePreparedLifecycle("alpha");
  assert(JSON.stringify(preparedLifecycle.states) === JSON.stringify(["prepared", "active", "paused"]), "prepare, prepared start, and stop must expose the exact three lifecycle states");
  assert(preparedLifecycle.materializations === 2, "prepare and birth must each materialize once while prepared start must not rematerialize VMs");
  assert(JSON.stringify(preparedLifecycle.serviceActions) === JSON.stringify(["prepare", "start", "stop", "uninstall"]), "prepared start must launch keepers without restarting the prepared API");
  assert(preparedLifecycle.copyRejected && preparedLifecycle.rebindRejected, "copy and rebind must reject a prepared experiment");
  assert(JSON.stringify(preparedLifecycle.birthArguments) === JSON.stringify(["society-birth", "Aster", "--role", "map the frontier"]), "birth must invoke the narrow existing birth operation with the exact optional role");
  assert(JSON.stringify(preparedLifecycle.birthEvents) === JSON.stringify([["society-birth", "Aster", "--role", "map the frontier"], "materialize"]), "birth must materialize only after the existing birth operation succeeds");
  assert(preparedLifecycle.pausedBirthRejected, "birth must reject a paused package");
  const pendingInvariant = await exercisePendingBirthInvariant("alpha");
  assert(pendingInvariant.copyBlocked && pendingInvariant.pausedStartBlocked && pendingInvariant.preparedStartBlocked, "pending births must block copy and both start paths");
  assert(pendingInvariant.status.includes("pending_births=unresolved"), "status must surface unresolved birth addresses");
  assert(pendingInvariant.prepareAllowed && pendingInvariant.birthResolved && pendingInvariant.finalState === "paused", "prepare and matching birth retry must resolve pending state without a new lifecycle");
  const failedBirth = await exerciseBirthFailure("alpha");
  assert(failedBirth.failedState === "failed", "materialization failure after birth must retain failed state");
  assert(failedBirth.recoveredState === "paused", "failed birth must remain recoverable through the supported recovery path");

  const snapshotRequests: string[] = [];
  const redirectedRequests: string[] = [];
  let redirectHealth = false;
  const redirectTarget = Bun.serve({
    hostname: controllerHost,
    port: 0,
    fetch(request) {
      redirectedRequests.push(new URL(request.url).pathname);
      return Response.json({ instanceId: "alpha" });
    },
  });
  const snapshotApi = Bun.serve({
    hostname: controllerHost,
    port: alphaPort,
    fetch(request) {
      assert(request.headers.get("authorization") === `Bearer ${alphaSecret.trim()}`, "observer capture must use only the instance substrate authority");
      const path = new URL(request.url).pathname;
      snapshotRequests.push(path);
      if (path === "/api/health" && redirectHealth) {
        return new Response(null, {
          status: 302,
          headers: { location: `http://${controllerHost}:${redirectTarget.port}/redirected` },
        });
      }
      const bodies: Record<string, unknown> = {
        "/api/health": { status: "ok", instanceId: "alpha" },
        "/api/society/world-evidence": {activeRunId:"fixture",runs:[{id:"fixture",manifest:{world:{id:"honest"}}}]},
        "/api/society/admin": { agents: [{ address: "agent-a", name: "Agent A" }], posts: [] },
        "/api/society/admin/agents/agent-a": { agent: { address: "agent-a", name: "Agent A" }, wakes: [], posts: [], transcripts: [] },
      };
      return path in bodies
        ? Response.json(bodies[path])
        : new Response("not found", { status: 404 });
    },
  });
  try {
    const capture = await captureObserver("alpha");
    assert(capture.status === 0, capture.error);
    assert(JSON.stringify(snapshotRequests) === JSON.stringify(["/api/health", "/api/society/admin", "/api/society/admin/agents/agent-a", "/api/society/world-evidence"]), "observer capture must verify instance identity and fetch each exact agent detail");
    redirectHealth = true;
    const redirectedCapture = await captureObserver("alpha");
    assert(redirectedCapture.status !== 0, "observer capture must reject HTTP redirects");
    assert(redirectedRequests.length === 0, "Bearer authority must never reach a redirect target");
  } finally {
    snapshotApi.stop(true);
    redirectTarget.stop(true);
  }
  const observer = JSON.parse(readFileSync(join(alpha, "evidence", "observer.json"), "utf8"));
  assert(observer.instanceId === "alpha" && typeof observer.capturedAt === "string", "observer snapshot must record instance identity and capture time");
  assert(observer.overview.agents[0].address === "agent-a" && observer.agentsByAddress["agent-a"].agent.address === "agent-a", "observer snapshot must preserve exact overview and per-agent projections");

  const vmKit = join(alpha, "agents", "kits", "agent-vm");
  const fakeSandbox = join(root, "fake-sandbox", "sandbox");
  const vmPidfile = join(root, "fake-sandbox", "state", "alpha-agent-vm.pid");
  mkdirSync(vmKit);
  mkdirSync(join(root, "fake-sandbox", "state"), { recursive: true });
  const sleepingVm = Bun.spawn(["sleep", "30"], { stdout: "ignore", stderr: "ignore" });
  writeFileSync(vmPidfile, `${sleepingVm.pid}\n`);
  assert(await waitForVmQuiescence("alpha", fakeSandbox, 0.1) !== 0, "VM quiescence timeout must fail while an experiment VM is alive");
  sleepingVm.kill();
  await sleepingVm.exited;
  assert(await waitForVmQuiescence("alpha", fakeSandbox, 0.1) === 0, "VM quiescence must pass only after every experiment VM exits");
  rmSync(vmKit, { recursive: true, force: true });
  rmSync(join(root, "fake-sandbox"), { recursive: true, force: true });

  unlinkSync(join(alpha, ".paused"));
  writeFileSync(join(alpha, ".prepared"), "prepared\n");
  const failedCaptureStop = await stopWithFailedCapture("alpha");
  assert(failedCaptureStop !== 0, "stop fixture must fail when observer capture fails");
  assert(run("status", "alpha").includes("state=failed"), "failed stop must retain explicit failed state");
  assert((await exerciseRecovery("alpha", false)).state === "paused", "failed stop must use the supported recovery path");

  const observerPath = join(alpha, "evidence", "observer.json");
  const validObserver = readFileSync(observerPath, "utf8");
  writeFileSync(observerPath, JSON.stringify({ ...observer, instanceId: "wrong-source" }) + "\n");
  const invalidCopy = spawnSync("./fugue", ["experiment", "copy", "alpha", "invalid-copy"], { env, encoding: "utf8" });
  assert(invalidCopy.status !== 0 && invalidCopy.stderr.includes("identity does not match"), "copy must reject an observer snapshot that does not match its source package");
  assert(!existsSync(join(experiments, "invalid-copy")) && !existsSync(join(gateways, "invalid-copy")), "invalid copied evidence must not expose a target package or binding");
  writeFileSync(observerPath, validObserver);

  const alphaKit = join(alpha, "agents", "kits", "agent-a");
  mkdirSync(alphaKit);
  const alphaNode = {
    address: "agent-a",
    apiBaseUrl: `http://${vmBridgeHost}:${alphaPort}`,
    runner: { defaultModel: "luna", defaultEffort: "high" },
  };
  writeFileSync(join(alphaKit, "node.json"), JSON.stringify(alphaNode) + "\n");
  writeFileSync(join(alphaKit, "SPAWN.md"), "agent-a\n");
  const hiddenPendingKit = join(alpha, "agents", "kits", ".agent-a.pending");
  mkdirSync(hiddenPendingKit);
  const partialRecovery = await exercisePartialMaterializeRecovery("alpha", "partial-agent");
  assert(!partialRecovery.agentNames.includes(".agent-a.pending"), "hidden birth staging directories must never become agent names");
  assert(partialRecovery.failedState === "failed" && partialRecovery.recoveredState === "paused", "partial materialization must remain failed until normal recovery completes");
  assert(partialRecovery.priorHome === "valid-prior-home", "recovery must preserve the last valid home when rematerialization fails");
  assert(partialRecovery.incompleteExported === false, "recovery must not export incomplete VM state as a durable checkpoint");
  rmSync(hiddenPendingKit, { recursive: true });
  const materializedHomes = await exerciseMaterializeHomes("alpha", "agent-a");
  assert(materializedHomes.firstArchiveMembers.includes("society/node.json") && materializedHomes.firstArchiveMembers.includes("society/SPAWN.md"), "first materialization must import the package kit as the coder society directory");
  assert(materializedHomes.firstNodeApiBaseUrl === `http://${vmBridgeHost}:${alphaPort}`, "first materialization must preserve the kit node endpoint in the imported home");
  assert(!materializedHomes.temporaryArchiveExists, "first materialization must remove its temporary coder-home archive");
  assert(!materializedHomes.temporaryArchive.startsWith(join(alpha, "agents", "homes")), "first materialization must keep its temporary archive outside durable package state");
  assert(materializedHomes.checkpointImport.endsWith("agents/homes/agent-a.tar.gz"), "later materialization must prefer the durable home checkpoint");
  assert(JSON.stringify(materializedHomes.postImportActions) === JSON.stringify([["sync-openai-auth", "setup-subagents"], ["sync-openai-auth", "setup-subagents"]]), "materialization must refresh host-provided model auth after every home import and before runner policy setup");

  const copySecretTarget = join(root, "copy-secret-target.key");
  writeFileSync(copySecretTarget, "outside-copy-authority\n");
  mkdirSync(join(secrets, "beta"), { recursive: true });
  symlinkSync(copySecretTarget, join(secrets, "beta", "substrate.key"));
  run("copy", "alpha", "beta");
  const beta = join(experiments, "beta");
  const betaConfig = readFileSync(join(beta, "fugue.toml"), "utf8");
  const betaSecret = readFileSync(join(secrets, "beta", "substrate.key"), "utf8");
  const betaPort = sectionPort(betaConfig, "api", "port");
  const betaBinding = JSON.parse(readFileSync(join(gateways, "beta", "binding.json"), "utf8"));
  assert(betaConfig.includes('instance_id = "beta"'), "copy must assign a new package identity");
  assert(alphaSecret !== betaSecret, "copy must create a fresh substrate secret");
  assert(!lstatSync(join(secrets, "beta", "substrate.key")).isSymbolicLink(), "copy must replace a symlinked stale target secret");
  assert(readFileSync(copySecretTarget, "utf8") === "outside-copy-authority\n", "copy must not overwrite a stale secret's symlink target");
  assert(JSON.stringify(betaBinding) === JSON.stringify({ instanceId: "beta", apiHost: controllerHost, apiPort: betaPort }), "copy must write a destination-owned endpoint binding");
  assert(!existsSync(join(beta, "Caddyfile")), "copy must exclude a legacy host gateway configuration");
  assert(readFileSync(join(alpha, "data", "core.sqlite")).equals(readFileSync(join(beta, "data", "core.sqlite"))), "copy must preserve durable society data");
  const sourceNodeAfterCopy = JSON.parse(readFileSync(join(alphaKit, "node.json"), "utf8"));
  const betaNode = JSON.parse(readFileSync(join(beta, "agents", "kits", "agent-a", "node.json"), "utf8"));
  assert(sourceNodeAfterCopy.apiBaseUrl === `http://${vmBridgeHost}:${alphaPort}`, "copy must preserve the source kit endpoint");
  assert(JSON.stringify(betaNode) === JSON.stringify(alphaNode), "copy must preserve kit data instead of treating node.json as endpoint authority");
  const restoredBetaHome = await exerciseMaterializeHomes("beta", "agent-a");
  assert(restoredBetaHome.firstNodeApiBaseUrl === `http://${vmBridgeHost}:${alphaPort}`, "copied home materialization fixture must retain its stale source node endpoint");
  const betaObserver = JSON.parse(readFileSync(join(beta, "evidence", "observer.json"), "utf8"));
  assert(betaObserver.instanceId === "beta", "copied observer evidence must be scoped to the target package");
  assert(betaObserver.capturedAt === observer.capturedAt && JSON.stringify(betaObserver.overview) === JSON.stringify(observer.overview) && JSON.stringify(betaObserver.agentsByAddress) === JSON.stringify(observer.agentsByAddress), "copy must change only the observer snapshot instance identity");

  const shimDir = join(root, "bin");
  const python = spawnSync("which", ["python3"], { encoding: "utf8" }).stdout.trim();
  assert(python.length > 0, "test requires python3");
  mkdirSync(shimDir);
  writeFileSync(
    join(shimDir, "python3"),
    `#!/bin/sh
case "$1" in
  *scripts/society_admin.py)
    shift
    printf 'config=%s\\nkey=%s\\ndirect_key=%s\\napi=%s\\nargs=%s\\n' "$FUGUE_CONFIG" "$FUGUE_SUBSTRATE_KEY_FILE" "$FUGUE_SUBSTRATE_KEY" "$FUGUE_API_URL" "$*"
    ;;
  *scripts/society_birth.py)
    shift
    mkdir -p "\${FUGUE_CONFIG%/*}/agents/kits/$1"
    printf '{}\n' > "\${FUGUE_CONFIG%/*}/agents/kits/$1/node.json"
    printf 'config=%s\\nkey=%s\\ndirect_key=%s\\napi=%s\\nargs=%s\\n' "$FUGUE_CONFIG" "$FUGUE_SUBSTRATE_KEY_FILE" "$FUGUE_SUBSTRATE_KEY" "$FUGUE_API_URL" "$*"
    ;;
  *) exec ${JSON.stringify(python)} "$@" ;;
esac
`,
  );
  chmodSync(join(shimDir, "python3"), 0o755);
  const society = spawnSync("./fugue", ["experiment", "society", "alpha", "status"], {
    env: { ...env, PATH: `${shimDir}:${process.env.PATH ?? ""}` },
    encoding: "utf8",
  });
  assert(society.status === 0, society.stderr || society.stdout);
  assert(society.stdout.includes(`config=${join(alpha, "fugue.toml")}`), "instance society command must select the package config");
  assert(society.stdout.includes(`key=${join(secrets, "alpha", "substrate.key")}`), "instance society command must select the package key");
  assert(society.stdout.includes("direct_key=\n"), "instance society command must discard inherited direct authority");
  assert(society.stdout.includes(`api=http://${controllerHost}:${alphaPort}`), "instance society command must derive the trusted package API URL");
  assert(society.stdout.includes("args=status"), "instance society command must forward society-admin arguments");
  writeFileSync(sandboxLog, "");
  unlinkSync(join(alpha, ".paused"));
  writeFileSync(join(alpha, ".prepared"), "prepared\n");
  const born = spawnSync("./fugue", ["experiment", "birth", "alpha", "Bram", "--role", "test the edge"], {
    env: { ...env, PATH: `${shimDir}:${process.env.PATH ?? ""}` },
    encoding: "utf8",
  });
  assert(born.status === 0, born.stderr || born.stdout);
  assert(born.stdout.includes(`config=${join(alpha, "fugue.toml")}`), "birth must select the prepared package config");
  assert(born.stdout.includes(`key=${join(secrets, "alpha", "substrate.key")}`), "birth must retain the package key-file boundary");
  assert(born.stdout.includes(`direct_key=${alphaSecret.trim()}`), "birth must provide the existing birth command its package authority");
  assert(born.stdout.includes(`api=http://${controllerHost}:${alphaPort}`), "birth must derive the trusted package API URL");
  assert(born.stdout.includes("args=Bram --role test the edge"), "birth must forward only the name and optional role");
  assert(readFileSync(sandboxLog, "utf8").includes("copy base alpha-Bram"), "birth must materialize the new agent VM before returning to prepared state");
  assert(existsSync(join(alpha, ".prepared")) && !existsSync(join(alpha, ".failed")), "successful birth must finish in prepared state");
  unlinkSync(join(alpha, ".prepared"));
  writeFileSync(join(alpha, ".paused"), "paused\n");

  unlinkSync(join(secrets, "beta", "substrate.key"));
  writeFileSync(join(beta, ".prepared"), "prepared\n");
  const conflictingStatus = spawnSync("./fugue", ["experiment", "status", "beta"], { env, encoding: "utf8" });
  assert(conflictingStatus.status !== 0 && conflictingStatus.stderr.includes("conflicting lifecycle markers"), "status must reject mutually present paused and prepared markers");
  unlinkSync(join(beta, ".prepared"));
  unlinkSync(join(beta, ".paused"));
  symlinkSync("fugue.toml", join(beta, ".paused"));
  const linkedStatus = spawnSync("./fugue", ["experiment", "status", "beta"], { env, encoding: "utf8" });
  assert(linkedStatus.status !== 0 && linkedStatus.stderr.includes("must not contain symlinks"), "status must reject a linked lifecycle marker at the package boundary");
  unlinkSync(join(beta, ".paused"));
  writeFileSync(join(beta, ".paused"), "paused\n");
  unlinkSync(join(experiments, "gamma", ".paused"));
  writeFileSync(join(experiments, "gamma", ".failed"), "start\n");
  const status = run("status");
  assert(status.includes("experiment=alpha state=paused") && status.includes("experiment=beta state=paused") && status.includes("experiment=gamma state=failed"), "status must report explicit failed lifecycle truth alongside paused packages");
  assert(!existsSync(join(secrets, "beta", "substrate.key")), "status must not create missing authority");
  console.log("experiment_packages_e2e=passed");
  console.log("packages=alpha,beta,gamma;ids=reserved-default;ports=bound-checked,concurrent-safe;bindings=host-owned,authority-checked;boundaries=config-and-tree-contained;lifecycle=paused-prepared-active-failed,recoverable,vm-quiescent;compute=typed-paired-config,paused-only,exact-rewrite;birth=prepared-failed-prepared,stable-key-retry,missing-key-rebuild,pending-start-copy-gated;materialize=kit-first,checkpoint-later,incomplete-export-skipped;observer=identity-checked,no-redirect,archived;copy=data-and-kits-preserved,evidence-target-scoped,secrets-fresh-no-follow;society=instance-scoped");
  console.log("scenario_cost scenario=experiment-packages llm_calls=0 input_tokens=0 output_tokens=0 cost_usd=0.00000000");
} finally {
  occupiedPort?.stop(true);
  rmSync(root, { recursive: true, force: true });
}

function run(...args: string[]): string {
  const result = spawnSync("./fugue", ["experiment", ...args], { env, encoding: "utf8" });
  if (result.status !== 0) throw new Error(result.stderr || result.stdout);
  return result.stdout;
}

async function runAsync(...args: string[]): Promise<string> {
  const child = Bun.spawn(["./fugue", "experiment", ...args], { env: { ...env, FUGUE_SERVER_BIN: resolve("target/debug/fugue-server") }, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (status !== 0) throw new Error(stderr || stdout);
  return stdout;
}

async function captureObserver(experimentId: string): Promise<{ status: number; error: string }> {
  const source = `import importlib.util\nspec=importlib.util.spec_from_file_location("experiment", ${JSON.stringify(resolve("scripts/experiment.py"))})\nmodule=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(module)\nmodule.capture_observer(module.Host(), ${JSON.stringify(experimentId)})`;
  const child = Bun.spawn(["python3", "-c", source], { env, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { status, error: stderr || stdout };
}

async function waitForVmQuiescence(experimentId: string, sandboxPath: string, timeout: number): Promise<number> {
  const source = `import importlib.util\nspec=importlib.util.spec_from_file_location("experiment", ${JSON.stringify(resolve("scripts/experiment.py"))})\nmodule=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(module)\nhost=module.Host()\nhost.sandbox=${JSON.stringify(sandboxPath)}\nmodule.wait_for_vms_quiescent(host, ${JSON.stringify(experimentId)}, ${timeout})`;
  const child = Bun.spawn(["python3", "-c", source], { env, stdout: "ignore", stderr: "ignore" });
  return child.exited;
}

async function stopWithFailedCapture(experimentId: string): Promise<number> {
  const source = `import importlib.util\nspec=importlib.util.spec_from_file_location("experiment", ${JSON.stringify(resolve("scripts/experiment.py"))})\nmodule=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(module)\nmodule.run_fugue=lambda *args,**kwargs: None\ndef fail(*args,**kwargs): raise RuntimeError("capture failed")\nmodule.capture_observer=fail\nmodule.operate(module.Host(), ${JSON.stringify(experimentId)}, "stop")`;
  const child = Bun.spawn(["python3", "-c", source], { env, stdout: "ignore", stderr: "ignore" });
  return child.exited;
}

async function exerciseBirthRetry(configPath: string, kitRoot: string): Promise<{ firstFailedWithPending: boolean; requestKeys: string[]; finalKey: string; pendingExists: boolean; localIncompleteFinalized: boolean; localIncompleteKey: string; failedPublicationLeftNoFinal: boolean; exclusiveWritePreserved: boolean }> {
  const source = `import importlib.util,json,os,pathlib,sys\nspec=importlib.util.spec_from_file_location("society_birth", ${JSON.stringify(resolve("scripts/society_birth.py"))})\nmodule=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(module)\nkit_root=pathlib.Path(${JSON.stringify(kitRoot)})\nrequests=[]\ndef fake_post(base,key,path,body):\n if body["address"]=="retry-agent" and requests: assert body==requests[0]\n requests.append(dict(body))\n return {"actorId":"actor-retry","address":body["address"],"key":body["key"]}\nmodule.post=fake_post\noriginal_finalize=module.finalize_pending\ndef crash_after_commit(*args): raise RuntimeError("crash after commit")\nmodule.finalize_pending=crash_after_commit\nos.environ["FUGUE_CONFIG"]=${JSON.stringify(configPath)}\nos.environ["FUGUE_SUBSTRATE_KEY"]="substrate-test-key"\nsys.argv=["society-birth","Retry Agent","--kit-dir",str(kit_root)]\ntry: module.main()\nexcept RuntimeError: pass\npending=kit_root/".retry-agent.pending"\nfirst_failed=pending.is_dir() and not (kit_root/"retry-agent").exists()\nmodule.finalize_pending=original_finalize\nsys.argv=["society-birth","Retry Agent","--kit-dir",str(kit_root)]\nmodule.main()\ndestination=kit_root/"retry-agent"\nincomplete=kit_root/".local-incomplete.pending"\nincomplete.mkdir()\n(incomplete/"partial-file").write_text("pre-api crash")\nsys.argv=["society-birth","Local Incomplete","--kit-dir",str(kit_root)]\nmodule.main()\nlocal_destination=kit_root/"local-incomplete"\npublication_dir=kit_root/"publication"\npublication_dir.mkdir()\npartial_final=publication_dir/"partial.key"\noriginal_fsync=module.os.fsync\ndef fail_fsync(fd): raise OSError("forced fsync failure")\nmodule.os.fsync=fail_fsync\ntry: module.write_durable(partial_final,"partial\\n",0o600)\nexcept OSError: pass\nfinally: module.os.fsync=original_fsync\nfailed_publication_left_no_final=not os.path.lexists(partial_final)\nexisting=publication_dir/"existing.key"\nexisting.write_text("original\\n")\ntry: module.write_durable(existing,"replacement\\n",0o600)\nexcept FileExistsError: pass\nexclusive_write_preserved=existing.read_text()=="original\\n"\nprint(json.dumps({"firstFailedWithPending":first_failed,"requestKeys":[item["key"] for item in requests if item["address"]=="retry-agent"],"finalKey":(destination/"agent.key").read_text().strip(),"pendingExists":pending.exists(),"localIncompleteFinalized":local_destination.is_dir() and not incomplete.exists(),"localIncompleteKey":(local_destination/"agent.key").read_text().strip(),"failedPublicationLeftNoFinal":failed_publication_left_no_final,"exclusiveWritePreserved":exclusive_write_preserved}))`;
  const child = Bun.spawn(["python3", "-c", source], { env, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (status !== 0) throw new Error(stderr || stdout);
  return JSON.parse(stdout.trim().split("\n").at(-1)!);
}

async function exerciseRecovery(experimentId: string, failQuiescence: boolean): Promise<{ state: string; serviceActions: string[]; quiescenceChecks: number }> {
  const source = `import importlib.util,json\nspec=importlib.util.spec_from_file_location("experiment", ${JSON.stringify(resolve("scripts/experiment.py"))})\nmodule=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(module)\nhost=module.Host()\nevents=[]\nmodule.run_fugue=lambda host,experiment,*args,**kwargs: events.append(args[-1])\ndef wait(*args,**kwargs):\n events.append("quiescent")\n if ${failQuiescence ? "True" : "False"} and events.count("quiescent")==1: raise RuntimeError("still running")\nmodule.wait_for_vms_quiescent=wait\nmodule.capture_observer=lambda *args,**kwargs: events.append("capture")\nmodule.export_homes=lambda *args,**kwargs: events.append("export")\ntry: module.recover(host,${JSON.stringify(experimentId)})\nexcept RuntimeError: pass\npackage=host.package(${JSON.stringify(experimentId)})\nprint(json.dumps({"state":module.package_state(package),"serviceActions":[item for item in events if item in ("stop","uninstall","verify-stopped")],"quiescenceChecks":events.count("quiescent")}))`;
  const child = Bun.spawn(["python3", "-c", source], { env, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (status !== 0) throw new Error(stderr || stdout);
  return JSON.parse(stdout.trim().split("\n").at(-1)!);
}

async function exercisePartialMaterializeRecovery(experimentId: string, agent: string): Promise<{ agentNames: string[]; failedState: string; recoveredState: string; priorHome: string; incompleteExported: boolean }> {
  const source = `import importlib.util,json,pathlib,shutil\nspec=importlib.util.spec_from_file_location("experiment", ${JSON.stringify(resolve("scripts/experiment.py"))})\nmodule=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(module)\nhost=module.Host()\npackage=host.package(${JSON.stringify(experimentId)})\nagent=${JSON.stringify(agent)}\nkit=package/"agents/kits"/agent\nkit.mkdir()\n(kit/"node.json").write_text("{}\\n")\nhome=package/"agents/homes"/(agent+".tar.gz")\nhome.write_bytes(b"valid-prior-home")\nagent_names=module.agent_names(package)\nexports=[]\nphase="materialize"\ndef fake_sandbox(host,*args,**kwargs):\n if args[0]=="setup-subagents" and args[1].endswith(agent): raise RuntimeError("setup failed")\n if args[0]=="export-home":\n  exports.append(args[1])\n  pathlib.Path(args[2]).write_bytes(b"incomplete-vm-home")\n return ""\nmodule.sandbox=fake_sandbox\nmodule.environments=lambda host: set() if phase=="materialize" else {${JSON.stringify(`${experimentId}-${agent}`)}}\n(package/".paused").unlink()\n(package/".prepared").write_text("prepared\\n")\nmodule.begin_transition(package,"prepared","birth")\ntry: module.materialize(host,${JSON.stringify(experimentId)})\nexcept RuntimeError: pass\nfailed_state=module.package_state(package)\nphase="recover"\nmodule.run_fugue=lambda *args,**kwargs: None\nmodule.wait_for_vms_quiescent=lambda *args,**kwargs: None\nmodule.capture_observer=lambda *args,**kwargs: None\nmodule.recover(host,${JSON.stringify(experimentId)})\nresult={"agentNames":agent_names,"failedState":failed_state,"recoveredState":module.package_state(package),"priorHome":home.read_text(),"incompleteExported":${JSON.stringify(`${experimentId}-${agent}`)} in exports}\nshutil.rmtree(kit)\nhome.unlink()\nmarker=module.materialization_marker(package,agent)\nmarker.unlink()\ntry: marker.parent.rmdir()\nexcept OSError: pass\n(package/"agents/homes/agent-a.tar.gz").unlink(missing_ok=True)\nprint(json.dumps(result))`;
  const child = Bun.spawn(["python3", "-c", source], { env, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (status !== 0) throw new Error(stderr || stdout);
  return JSON.parse(stdout.trim().split("\n").at(-1)!);
}

async function exerciseMaterializeHomes(experimentId: string, agent: string): Promise<{ firstArchiveMembers: string[]; firstNodeApiBaseUrl: string; temporaryArchive: string; temporaryArchiveExists: boolean; checkpointImport: string; postImportActions: string[][] }> {
  const source = `import importlib.util,json,pathlib,tarfile\nspec=importlib.util.spec_from_file_location("experiment", ${JSON.stringify(resolve("scripts/experiment.py"))})\nmodule=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(module)\nhost=module.Host()\nimports=[]\nactions=[]\nfirst_members=[]\nfirst_node_url=None\ndef fake_sandbox(host,*args,**kwargs):\n global first_node_url\n actions.append(list(args))\n if args[0]=="import-home":\n  imports.append(args[2])\n  if len(imports)==1:\n   with tarfile.open(args[2],"r:gz") as archive:\n    first_members.extend(archive.getnames())\n    first_node_url=json.load(archive.extractfile("society/node.json"))["apiBaseUrl"]\n return ""\nmodule.sandbox=fake_sandbox\nmodule.environments=lambda host: set()\nmodule.materialize(host,${JSON.stringify(experimentId)})\ntemporary=imports[0]\ncheckpoint=host.package(${JSON.stringify(experimentId)})/"agents/homes"/${JSON.stringify(`${agent}.tar.gz`)}\ncheckpoint.write_bytes(b"checkpoint")\nmodule.materialize(host,${JSON.stringify(experimentId)})\ncheckpoint.unlink()\npost_import=[]\nfor index,action in enumerate(actions):\n if action[0]=="import-home": post_import.append([actions[index+1][0],actions[index+2][0]])\nprint(json.dumps({"firstArchiveMembers":first_members,"firstNodeApiBaseUrl":first_node_url,"temporaryArchive":temporary,"temporaryArchiveExists":pathlib.Path(temporary).exists(),"checkpointImport":imports[-1],"postImportActions":post_import}))`;
  const child = Bun.spawn(["python3", "-c", source], { env, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (status !== 0) throw new Error(stderr || stdout);
  return JSON.parse(stdout.trim().split("\n").at(-1)!);
}

async function exercisePreparedLifecycle(experimentId: string): Promise<{ states: string[]; materializations: number; serviceActions: string[]; copyRejected: boolean; rebindRejected: boolean; birthArguments: string[]; birthEvents: unknown[]; pausedBirthRejected: boolean }> {
  const source = `import importlib.util,json\nspec=importlib.util.spec_from_file_location("experiment", ${JSON.stringify(resolve("scripts/experiment.py"))})\nmodule=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(module)\nhost=module.Host()\nevents=[]\nmodule.capture_world=lambda *args: None\nmodule.materialize=lambda *args: events.append("materialize")\nmodule.run_fugue=lambda host,experiment,*args,**kwargs: events.append(list(args))\nmodule.wait_for_vms_quiescent=lambda *args,**kwargs: events.append("quiescent")\nmodule.capture_observer=lambda *args,**kwargs: events.append("capture")\nmodule.export_homes=lambda *args,**kwargs: events.append("export")\npackage=host.package(${JSON.stringify(experimentId)})\nmodule.operate(host,${JSON.stringify(experimentId)},"prepare")\nstates=[module.package_state(package)]\nbirth_start=len(events)\nmodule.birth(host,${JSON.stringify(experimentId)},"Aster","map the frontier")\nbirth_events=events[birth_start:]\nbirth_arguments=birth_events[0]\ncopy_rejected=False\nrebind_rejected=False\ntry: module.copy_experiment(host,${JSON.stringify(experimentId)},"prepared-copy")\nexcept RuntimeError: copy_rejected=True\ntry: module.rebind(host,${JSON.stringify(experimentId)})\nexcept RuntimeError: rebind_rejected=True\nmodule.operate(host,${JSON.stringify(experimentId)},"start")\nstates.append(module.package_state(package))\nmodule.operate(host,${JSON.stringify(experimentId)},"stop")\nstates.append(module.package_state(package))\npaused_birth_rejected=False\ntry: module.birth(host,${JSON.stringify(experimentId)},"Cleo",None)\nexcept RuntimeError: paused_birth_rejected=True\nservice_actions=[item[-1] for item in events if isinstance(item,list) and item[-1] in ("prepare","start","stop","uninstall")]\nprint(json.dumps({"states":states,"materializations":events.count("materialize"),"serviceActions":service_actions,"copyRejected":copy_rejected,"rebindRejected":rebind_rejected,"birthArguments":birth_arguments,"birthEvents":birth_events,"pausedBirthRejected":paused_birth_rejected}))`;
  const child = Bun.spawn(["python3", "-c", source], { env, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (status !== 0) throw new Error(stderr || stdout);
  return JSON.parse(stdout);
}

async function exercisePendingBirthInvariant(experimentId: string): Promise<{ copyBlocked: boolean; pausedStartBlocked: boolean; preparedStartBlocked: boolean; status: string; prepareAllowed: boolean; birthResolved: boolean; finalState: string }> {
  const source = `import contextlib,importlib.util,io,json,shutil\nspec=importlib.util.spec_from_file_location("experiment", ${JSON.stringify(resolve("scripts/experiment.py"))})\nmodule=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(module)\nhost=module.Host()\npackage=host.package(${JSON.stringify(experimentId)})\npending=package/"agents/kits/.unresolved.pending"\npending.mkdir()\nmodule.capture_world=lambda *args: None\nmodule.materialize=lambda *args,**kwargs: None\ndef run(host,experiment,*args,**kwargs):\n if args[0]=="society-birth": shutil.rmtree(pending)\nmodule.run_fugue=run\ncopy_blocked=False\npaused_start_blocked=False\nprepared_start_blocked=False\ntry: module.copy_experiment(host,${JSON.stringify(experimentId)},"pending-copy")\nexcept RuntimeError as error: copy_blocked="run experiment prepare" in str(error)\ntry: module.operate(host,${JSON.stringify(experimentId)},"start")\nexcept RuntimeError as error: paused_start_blocked="rerun experiment birth" in str(error)\noutput=io.StringIO()\nwith contextlib.redirect_stdout(output): module.status(host,${JSON.stringify(experimentId)})\nmodule.operate(host,${JSON.stringify(experimentId)},"prepare")\nprepare_allowed=module.package_state(package)=="prepared"\ntry: module.operate(host,${JSON.stringify(experimentId)},"start")\nexcept RuntimeError as error: prepared_start_blocked="rerun experiment birth" in str(error)\nmodule.birth(host,${JSON.stringify(experimentId)},"Unresolved",None)\nbirth_resolved=not pending.exists() and module.package_state(package)=="prepared"\n(package/".prepared").replace(package/".paused")\nprint(json.dumps({"copyBlocked":copy_blocked,"pausedStartBlocked":paused_start_blocked,"preparedStartBlocked":prepared_start_blocked,"status":output.getvalue(),"prepareAllowed":prepare_allowed,"birthResolved":birth_resolved,"finalState":module.package_state(package)}))`;
  const child = Bun.spawn(["python3", "-c", source], { env, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (status !== 0) throw new Error(stderr || stdout);
  return JSON.parse(stdout.trim().split("\n").at(-1)!);
}

async function exerciseBirthFailure(experimentId: string): Promise<{ failedState: string; recoveredState: string }> {
  const source = `import importlib.util,json\nspec=importlib.util.spec_from_file_location("experiment", ${JSON.stringify(resolve("scripts/experiment.py"))})\nmodule=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(module)\nhost=module.Host()\npackage=host.package(${JSON.stringify(experimentId)})\n(package/".paused").unlink()\n(package/".prepared").write_text("prepared\\n")\nmodule.run_fugue=lambda *args,**kwargs: None\ndef fail_materialize(*args,**kwargs): raise RuntimeError("materialization failed")\nmodule.materialize=fail_materialize\ntry: module.birth(host,${JSON.stringify(experimentId)},"Failure",None)\nexcept RuntimeError: pass\nfailed_state=module.package_state(package)\nmodule.run_fugue=lambda *args,**kwargs: None\nmodule.wait_for_vms_quiescent=lambda *args,**kwargs: None\nmodule.capture_observer=lambda *args,**kwargs: None\nmodule.export_homes=lambda *args,**kwargs: None\nmodule.recover(host,${JSON.stringify(experimentId)})\nprint(json.dumps({"failedState":failed_state,"recoveredState":module.package_state(package)}))`;
  const child = Bun.spawn(["python3", "-c", source], { env, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (status !== 0) throw new Error(stderr || stdout);
  return JSON.parse(stdout.trim().split("\n").at(-1)!);
}

function sectionPort(config: string, section: string, key: string): number {
  const match = config.match(new RegExp(`^\\[${section}\\][\\s\\S]*?^${key} = (\\d+)$`, "m"));
  assert(match, `missing [${section}] ${key}`);
  return Number(match[1]);
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}
