import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

type Resolver = (value: unknown) => void;

const fake = vi.hoisted(() => {
  const overviewPending = new Map<string, Resolver[]>();
  const detailPending = new Map<string, Resolver[]>();
  const enqueue = (pending: Map<string, Resolver[]>, key: string, resolve: Resolver) => {
    pending.set(key, [...(pending.get(key) ?? []), resolve]);
  };
  return {
    overviewPending,
    detailPending,
    runtime: {
      listLabExperiments: vi.fn(async (): Promise<Array<{
        id: string;
        state: "paused" | "prepared" | "active" | "failed";
        apiReachable: boolean;
        apiPort: number;
      }> | null> => [
        { id: "alpha", state: "paused", apiReachable: false, apiPort: 1 },
        { id: "beta", state: "prepared", apiReachable: false, apiPort: 4 },
        { id: "gamma", state: "paused", apiReachable: false, apiPort: 7 },
        { id: "delta", state: "failed", apiReachable: true, apiPort: 10 }
      ]),
      readLabExperimentOverview: vi.fn(
        (id: string) => new Promise((resolve) => enqueue(overviewPending, id, resolve))
      ),
      readLabExperimentAgent: vi.fn(
        (id: string, address: string) =>
          new Promise((resolve) => enqueue(detailPending, `${id}:${address}`, resolve))
      ),
      listLabExperimentAgentFiles: vi.fn(async () => ({ snapshotAt: null, files: [] })),
      readLabExperimentAgentTextFile: vi.fn(),
      labExperimentAgentFileUrl: vi.fn(() => "/download"),
      readSocietyAdminOverview: vi.fn(),
      readSocietyAdminAgent: vi.fn()
    }
  };
});

vi.mock("../runtime", () => ({ runtime: fake.runtime }));

import SocietyAdmin from "./SocietyAdmin.svelte";

afterEach(() => {
  fake.overviewPending.clear();
  fake.detailPending.clear();
  vi.clearAllMocks();
  document.body.innerHTML = "";
});

async function flush() {
  for (let index = 0; index < 7; index += 1) {
    await Promise.resolve();
    await tick();
  }
}

function agent(address: string, phase: "awake" | "sleeping" = "awake") {
  return {
    address,
    name: address,
    balance: 100,
    spentThisWindow: 0,
    lifecycle: {
      phase,
      wakeNumber: 5,
      nextWakeAt: phase === "sleeping" ? "2026-08-28T10:00:00Z" : undefined,
      updatedAt: "2026-08-28T09:30:00Z"
    }
  };
}

function overview(address: string, phase: "awake" | "sleeping" = "awake") {
  return {
    agents: [agent(address, phase)],
    bank: {
      treasuryAddress: "treasury",
      treasuryBalance: 0,
      totalMinted: 200,
      windowStart: "2026-08-24T00:00:00.000Z",
      windowEnd: "2026-08-31T00:00:00.000Z"
    },
    posts: [],
    ledger: []
  };
}

function detail(address: string, phase: "awake" | "sleeping" = "awake") {
  return { agent: agent(address, phase), wakes: [], posts: [], transcripts: [] };
}

function resolveLast(pending: Map<string, Resolver[]>, key: string, value: unknown) {
  pending.get(key)?.at(-1)?.(value);
}

async function mountAlpha() {
  const app = mount(SocietyAdmin, { target: document.body });
  await flush();
  resolveLast(fake.overviewPending, "alpha", overview("friday", "sleeping"));
  await flush();
  return app;
}

describe("society observer selection", () => {
  it("keeps the single-instance route when no observer catalog exists", async () => {
    fake.runtime.listLabExperiments.mockResolvedValueOnce(null);
    fake.runtime.readSocietyAdminOverview.mockResolvedValueOnce(overview("local-agent"));
    fake.runtime.readSocietyAdminAgent.mockResolvedValueOnce(detail("local-agent"));
    const app = mount(SocietyAdmin, { target: document.body });
    await flush();

    expect(fake.runtime.readSocietyAdminOverview).toHaveBeenCalledOnce();
    expect(fake.runtime.readLabExperimentOverview).not.toHaveBeenCalled();
    expect(document.querySelector('button[data-agent="local-agent"]')).not.toBeNull();
    expect(document.querySelector('select[aria-label="Society experiment"]')).toBeNull();
    unmount(app);
  });

  it("shows the current versioned Newspaper as a society view", async () => {
    fake.runtime.listLabExperiments.mockResolvedValueOnce(null);
    fake.runtime.readSocietyAdminAgent.mockResolvedValueOnce(detail("local-agent"));
    fake.runtime.readSocietyAdminOverview.mockResolvedValueOnce({
      ...overview("local-agent"),
      commons: { id: "space-1", slug: "commons", name: "Commons" },
      newspaper: {
        artifactId: "artifact-news-1",
        spaceId: "space-1",
        version: 3,
        content: "# Commons Newspaper\n\n## Source record\n\n- The public finding remains contested. [P4]",
        sections: {
          sourceRecord: [{ text: "The public finding remains contested.", postReferences: ["P4"] }],
          synthesisInterpretation: [],
          openQuestions: []
        },
        throughPostSequence: 4,
        throughPostReference: "P4",
        createdAt: "2026-08-31T09:00:00Z"
      }
    });
    const app = mount(SocietyAdmin, { target: document.body });
    await flush();

    (document.querySelector('button[data-view="newspaper"]') as HTMLButtonElement).click();
    await flush();

    const newspaper = document.querySelector('[data-testid="society-newspaper"]');
    expect(newspaper?.textContent).toContain("The public finding remains contested");
    expect(newspaper?.textContent).toContain("P4");
    expect(document.body.textContent).toContain("Version 3 · through P4");
    unmount(app);
  });

  it("scopes reads to the selected society and ignores a stale overview", async () => {
    const app = await mountAlpha();
    const selector = document.querySelector('select[aria-label="Society experiment"]') as HTMLSelectElement;

    selector.value = "beta";
    selector.dispatchEvent(new Event("change", { bubbles: true }));
    await flush();
    const loadingSelector = document.querySelector('select[aria-label="Society experiment"]') as HTMLSelectElement;
    loadingSelector.value = "gamma";
    loadingSelector.dispatchEvent(new Event("change", { bubbles: true }));
    await flush();

    resolveLast(fake.overviewPending, "gamma", overview("gamma-agent"));
    await flush();
    resolveLast(fake.overviewPending, "beta", overview("beta-agent"));
    await flush();

    expect(fake.runtime.readLabExperimentOverview.mock.calls.map(([id]) => id)).toEqual([
      "alpha",
      "beta",
      "gamma"
    ]);
    expect(document.querySelector('button[data-agent="gamma-agent"]')).not.toBeNull();
    expect(document.querySelector('button[data-agent="beta-agent"]')).toBeNull();
    expect((document.querySelector('select[aria-label="Society experiment"]') as HTMLSelectElement).value).toBe("gamma");
    unmount(app);
  });

  it("marks an unreachable prepared society and does not request snapshot files", async () => {
    const app = await mountAlpha();
    const selector = document.querySelector('select[aria-label="Society experiment"]') as HTMLSelectElement;
    const preparedOption = selector.querySelector('option[value="beta"]') as HTMLOptionElement;

    expect(preparedOption.dataset.unavailable).toBe("true");
    expect(preparedOption.textContent).toMatch(/unavailable/i);

    selector.value = "beta";
    selector.dispatchEvent(new Event("change", { bubbles: true }));
    await flush();
    resolveLast(fake.overviewPending, "beta", overview("beta-agent"));
    await flush();
    (document.querySelector('button[data-agent="beta-agent"]') as HTMLButtonElement).click();
    await flush();
    resolveLast(fake.detailPending, "beta:beta-agent", detail("beta-agent"));
    await flush();

    expect(document.querySelector("section.files")?.getAttribute("data-snapshot-available")).toBe("false");
    expect(fake.runtime.listLabExperimentAgentFiles).not.toHaveBeenCalled();
    unmount(app);
  });

  it("keeps a failed package visible without live reads and invalidates a pending overview", async () => {
    const app = await mountAlpha();
    const selector = document.querySelector('select[aria-label="Society experiment"]') as HTMLSelectElement;
    selector.value = "beta";
    selector.dispatchEvent(new Event("change", { bubbles: true }));
    await flush();

    const loadingSelector = document.querySelector('select[aria-label="Society experiment"]') as HTMLSelectElement;
    const failedOption = loadingSelector.querySelector('option[value="delta"]') as HTMLOptionElement;
    expect(failedOption.dataset.unavailable).toBe("true");
    expect(failedOption.textContent).toMatch(/failed.*unavailable/i);

    loadingSelector.value = "delta";
    loadingSelector.dispatchEvent(new Event("change", { bubbles: true }));
    await flush();
    resolveLast(fake.overviewPending, "beta", overview("beta-agent"));
    await flush();

    expect(fake.runtime.readLabExperimentOverview.mock.calls.map(([id]) => id)).toEqual([
      "alpha",
      "beta"
    ]);
    expect(document.querySelector('button[data-agent="beta-agent"]')).toBeNull();
    expect((document.querySelector('select[aria-label="Society experiment"]') as HTMLSelectElement).value).toBe("delta");
    expect(document.querySelector(".load-state")?.textContent).toMatch(/failed/i);
    expect(fake.runtime.listLabExperimentAgentFiles).not.toHaveBeenCalled();
    unmount(app);
  });

  it("ignores a stale agent response after the society changes", async () => {
    const app = await mountAlpha();
    (document.querySelector('button[data-agent="friday"]') as HTMLButtonElement).click();
    await flush();

    const selector = document.querySelector('select[aria-label="Society experiment"]') as HTMLSelectElement;
    selector.value = "gamma";
    selector.dispatchEvent(new Event("change", { bubbles: true }));
    await flush();
    resolveLast(fake.overviewPending, "gamma", overview("new-agent"));
    await flush();

    resolveLast(fake.detailPending, "alpha:friday", detail("friday", "sleeping"));
    await flush();

    expect(document.querySelector(".agent-overview")).toBeNull();
    expect(document.querySelector('button[data-agent="new-agent"]')).not.toBeNull();
    unmount(app);
  });

  it("uses the explicit lifecycle pose in the sidebar and agent view", async () => {
    const app = await mountAlpha();
    const sidebarAvatar = document.querySelector(
      'button[data-agent="friday"] img.agent-avatar'
    ) as HTMLImageElement;
    expect(sidebarAvatar.getAttribute("src")).toBe("/assets/creatures/creature-07-asleep.png");

    (document.querySelector('button[data-agent="friday"]') as HTMLButtonElement).click();
    await flush();
    resolveLast(fake.detailPending, "alpha:friday", detail("friday", "sleeping"));
    await flush();

    expect((document.querySelector(".agent-heading img.agent-avatar") as HTMLImageElement).src).toBe(
      sidebarAvatar.src
    );
    unmount(app);
  });
});
