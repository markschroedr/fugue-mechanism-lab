import { spawn } from "node:child_process";

const suites = {
  deterministic: [
    "scripts/e2e_worker.ts",
    "scripts/e2e_mvp.ts",
    "scripts/e2e_entity_interactions.ts",
    "scripts/e2e_living_report.ts",
    "scripts/e2e_auth_surface.ts",
    "scripts/e2e_public_objection_core.ts",
    "scripts/e2e_alive_schedule.ts",
    "scripts/e2e_handles.ts",
    "scripts/e2e_society.ts",
    "scripts/e2e_newspaper.ts",
    "scripts/e2e_society_instances.ts",
    "scripts/e2e_experiment_packages.ts",
    "scripts/e2e_compute_backing.ts",
    "scripts/e2e_robinson.ts",
    "scripts/e2e_keeper.ts"
  ],
  live: [
    "scripts/e2e_agent_reply.ts",
    "scripts/e2e_ingest_synthesis.ts",
    "scripts/e2e_lesswrong_thread.ts",
    "scripts/e2e_lesswrong_refresh.ts",
    "scripts/e2e_incremental_synthesis.ts",
    "scripts/e2e_newspaper_live.ts",
    "scripts/e2e_public_objection.ts",
    "scripts/e2e_synthesis_crash.ts"
  ]
} as const;

const suiteName = process.argv[2];
if (suiteName !== "deterministic" && suiteName !== "live") {
  throw new Error("usage: bun run scripts/run_e2e_suite.ts <deterministic|live>");
}

const costLines: string[] = [];
const scenarios: readonly string[] = process.env.FUGUE_PRIVATE_E2E && suiteName === "deterministic"
  ? [...suites[suiteName], process.env.FUGUE_PRIVATE_E2E]
  : suites[suiteName];
for (const scenario of scenarios) {
  const env = { ...process.env, FUGUE_REQUIRE_SCENARIO_COST: "1" };
  if (suiteName === "deterministic") {
    delete env.OPENAI_API_KEY;
    delete env.OPENROUTER_API_KEY;
    env.FUGUE_EXPECT_ZERO_LLM = "1";
  }
  const output = await runScenario(scenario, env);
  const costLine = output
    .split("\n")
    .find((line) => line.startsWith("scenario_cost scenario=") && line.includes(" llm_calls="));
  if (!costLine) throw new Error(`${scenario} did not print its required scenario_cost line`);
  costLines.push(costLine);
}

const totals = costLines.reduce(
  (sum, line) => {
    const values = Object.fromEntries(line.split(" ").slice(1).map((field) => field.split("=", 2)));
    sum.llmCalls += Number(values.llm_calls);
    sum.inputTokens += Number(values.input_tokens);
    sum.outputTokens += Number(values.output_tokens);
    sum.costUsd += Number(values.cost_usd);
    return sum;
  },
  { llmCalls: 0, inputTokens: 0, outputTokens: 0, costUsd: 0 }
);

console.log(`scenario_cost_summary suite=${suiteName}`);
for (const line of costLines) console.log(line);
console.log(
  `scenario_cost_total suite=${suiteName} scenarios=${costLines.length} llm_calls=${totals.llmCalls} input_tokens=${totals.inputTokens} output_tokens=${totals.outputTokens} cost_usd=${totals.costUsd.toFixed(8)}`
);

async function runScenario(scenario: string, env: NodeJS.ProcessEnv): Promise<string> {
  const child = spawn("bun", ["run", scenario], { env, stdio: ["ignore", "pipe", "pipe"] });
  let output = "";
  child.stdout.on("data", (chunk) => {
    const text = chunk.toString();
    output += text;
    process.stdout.write(text);
  });
  child.stderr.on("data", (chunk) => {
    const text = chunk.toString();
    output += text;
    process.stderr.write(text);
  });
  const exitCode = await new Promise<number>((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code) => resolve(code ?? 1));
  });
  if (exitCode !== 0) throw new Error(`${scenario} exited with status ${exitCode}`);
  return output;
}
