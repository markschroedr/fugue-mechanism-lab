import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

const fake = vi.hoisted(() => {
  const calls: Record<string, number> = {
    health: 0, readCosts: 0, authSession: 0, listSpaces: 0,
    readDoc: 0, readReport: 0, readRefreshSchedule: 0, readFalsifiers: 0, readSources: 0, listReview: 0,
    submitInput: 0, synthesize: 0
  };
  let reviewHasSource = false;
  let reportVersion = 0;
  const count = (name: string) => { calls[name] = (calls[name] ?? 0) + 1; };
  return {
    calls,
    setReviewHasSource(value: boolean) { reviewHasSource = value; },
    setReportVersion(value: number) { reportVersion = value; },
    runtime: {
      kind: "http",
      health: async () => { count("health"); return { status: "ok", engine: "test", storage: "test", runtime: "http" }; },
      readCosts: async () => { count("readCosts"); return {
        overall: { llmCalls: 0, inputTokens: 0, outputTokens: 0, costUsd: 0 },
        spaces: [], stages: [], servedModels: []
      }; },
      authSession: async () => { count("authSession"); return { actorId: "actor-1", username: "reader" }; },
      listSpaces: async () => { count("listSpaces"); return [{ id: "space-1", title: "Quiet report", projection: "belief", neighbors: [] } as const]; },
      readDoc: async () => { count("readDoc"); return { spaceId: "space-1", title: "Quiet report", version: 1, compiledAt: "now", nodes: [] }; },
      readReport: async () => { count("readReport"); return reportVersion ? { artifactId: `report-${reportVersion}`, spaceId: "space-1", version: reportVersion, content: "## Source record\n\n## Synthesis interpretation\n\n## Cruxes", mode: "full", createdAt: "now", provenanceAnchors: [] } : null; },
      readRefreshSchedule: async () => { count("readRefreshSchedule"); return null; },
      readFalsifiers: async () => { count("readFalsifiers"); return []; },
      readSources: async () => { count("readSources"); return []; },
      listReview: async () => { count("listReview"); return reviewHasSource ? [{ id: "review-1", kind: "ingest", summary: "Ready", sourceArtifactId: "source-1" }] : []; },
      submitInput: async () => { count("submitInput"); return { artifactId: "source-1", jobId: "job-import" }; },
      synthesize: async () => { count("synthesize"); return { jobId: "job-synthesis" }; }
    }
  };
});

vi.mock("./runtime", () => ({ runtime: fake.runtime }));

import App from "./App.svelte";
import { review } from "./lib/review.svelte";
import { workspace } from "./lib/workspace.svelte";

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
  for (const key of Object.keys(fake.calls)) fake.calls[key] = 0;
  fake.setReviewHasSource(false);
  fake.setReportVersion(0);
});

async function flushEffects() {
  for (let turn = 0; turn < 6; turn += 1) {
    await Promise.resolve();
    await tick();
  }
}
function totalRequests() { return Object.values(fake.calls).reduce((sum, count) => sum + count, 0); }

describe("report network budget", () => {
  it("keeps one-minute idle, import, and synthesis traffic within explicit bounds", async () => {
    vi.useFakeTimers();
    const app = mount(App, { target: document.body });
    await flushEffects();

    await vi.advanceTimersByTimeAsync(60_000);
    await flushEffects();
    const idleRequests = totalRequests();
    expect(idleRequests).toBeGreaterThanOrEqual(18);
    expect(idleRequests).toBeLessThanOrEqual(21);
    expect(fake.calls.health).toBe(1);
    expect(fake.calls.readDoc).toBe(1);
    expect(fake.calls.readReport).toBe(1);
    expect(fake.calls.readRefreshSchedule).toBe(1);
    expect(fake.calls.readFalsifiers).toBe(0);
    expect(fake.calls.readSources).toBe(1);

    const beforeImport = totalRequests();
    const receipt = await workspace.submitInput("text", "https://www.lesswrong.com/posts/fixture");
    const importWait = review.waitForSource(receipt.artifactId, () => workspace.refreshSources());
    await vi.advanceTimersByTimeAsync(60_000);
    await flushEffects();
    const importRequests = totalRequests() - beforeImport;
    expect(importRequests).toBeGreaterThanOrEqual(90);
    expect(importRequests).toBeLessThanOrEqual(100);
    fake.setReviewHasSource(true);
    await vi.advanceTimersByTimeAsync(1_500);
    await importWait;

    const beforeSynthesis = totalRequests();
    const synthesis = workspace.synthesize("full");
    await flushEffects();
    await vi.advanceTimersByTimeAsync(60_000);
    await flushEffects();
    const synthesisRequests = totalRequests() - beforeSynthesis;
    expect(synthesisRequests).toBeGreaterThanOrEqual(50);
    expect(synthesisRequests).toBeLessThanOrEqual(55);
    fake.setReportVersion(1);
    await vi.advanceTimersByTimeAsync(1_500);
    await synthesis;

    const beforeUnmount = totalRequests();
    unmount(app);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(totalRequests()).toBe(beforeUnmount);

    console.log(`network_budget_idle_60s=${idleRequests}`);
    console.log(`network_budget_import_60s=${importRequests}`);
    console.log(`network_budget_synthesis_60s=${synthesisRequests}`);
  });
});
