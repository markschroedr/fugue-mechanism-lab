import { describe, expect, it } from "vitest";
import { buildSocietyTimeline } from "./society-timeline";

describe("society timeline", () => {
  it("orders recorded entries newest-first and deduplicates stable records", () => {
    const overview = {
      agents: [],
      bank: {
        treasuryAddress: "treasury",
        treasuryBalance: 0,
        totalMinted: 0,
        windowStart: "2026-08-01T00:00:00Z",
        windowEnd: "2026-08-02T00:00:00Z"
      },
      posts: [
        { id: "post-1", sequence: 1, reference: "C-1", spaceId: "commons", authorAddress: "ada", kind: "note" as const, headline: "Recorded post", sealed: false, recipientAddresses: [], attachedObols: 0, createdAt: "2026-08-29T10:00:00Z" },
        { id: "post-1", sequence: 1, reference: "C-1", spaceId: "commons", authorAddress: "ada", kind: "note" as const, headline: "Recorded post", sealed: false, recipientAddresses: [], attachedObols: 0, createdAt: "2026-08-29T10:00:00Z" }
      ],
      ledger: [
        { action: "paid", fromAddress: "ada", toAddress: "treasury", amount: 1, reason: "wake", createdAt: "2026-08-29T11:00:00Z" }
      ]
    };
    const detail = {
      agent: { address: "ada", name: "ada", balance: 0, spentThisWindow: 0 },
      wakes: [
        { wakeNumber: 3, model: "sol", inputTokens: 1, cachedInputTokens: 0, outputTokens: 1, chargedObols: 1, ending: "sleep", endedAt: "2026-08-29T12:00:00Z" }
      ],
      posts: [],
      transcripts: []
    };

    const timeline = buildSocietyTimeline(overview, [detail, detail]);

    expect(timeline.map((entry) => entry.key)).toEqual([
      "wake:ada:3",
      "ledger:0:2026-08-29T11:00:00Z",
      "post:post-1"
    ]);
  });
});
