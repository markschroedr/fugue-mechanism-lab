export type ScenarioCostTotals = {
  llmCalls: number;
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
};

type CostSummary = { overall: ScenarioCostTotals };

export async function printScenarioCost(baseUrl: string, scenario: string): Promise<ScenarioCostTotals> {
  const response = await fetch(`${baseUrl}/api/costs`);
  if (!response.ok) {
    throw new Error(`could not read scenario costs: ${response.status} ${await response.text()}`);
  }
  const totals = ((await response.json()) as CostSummary).overall;
  console.log(
    `scenario_cost scenario=${scenario} llm_calls=${totals.llmCalls} input_tokens=${totals.inputTokens} output_tokens=${totals.outputTokens} cost_usd=${totals.costUsd.toFixed(8)}`
  );
  return totals;
}

export async function printScenarioCostAtTeardown(baseUrl: string, scenario: string): Promise<void> {
  try {
    const totals = await printScenarioCost(baseUrl, scenario);
    if (process.env.FUGUE_EXPECT_ZERO_LLM === "1" && (totals.llmCalls !== 0 || totals.inputTokens !== 0 || totals.outputTokens !== 0 || totals.costUsd !== 0)) {
      console.error(`deterministic scenario ${scenario} recorded LLM usage`);
      process.exitCode = 1;
    }
  } catch (error) {
    console.error(`scenario_cost scenario=${scenario} unavailable=${error instanceof Error ? error.message : String(error)}`);
    if (process.env.FUGUE_REQUIRE_SCENARIO_COST === "1") process.exitCode = 1;
  }
}
