import { describe, expect, it, vi } from "vitest";
import type { ReviewItem } from "./types";

const fake = vi.hoisted(() => ({
  responses: [] as Array<Promise<ReviewItem[]>>
}));

vi.mock("../runtime", () => ({
  runtime: {
    listReview: () => fake.responses.shift() ?? Promise.resolve([])
  }
}));

import { ReviewState } from "./review.svelte";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
}

describe("ReviewState request ordering", () => {
  it("does not let an older poll overwrite a post-accept refresh", async () => {
    const older = deferred<ReviewItem[]>();
    const newer = deferred<ReviewItem[]>();
    fake.responses = [older.promise, newer.promise];
    const state = new ReviewState();

    const oldLoad = state.load();
    const newLoad = state.load();
    const arriving = { id: "review-new", kind: "ingest", summary: "New input" } satisfies ReviewItem;
    newer.resolve([arriving]);
    await newLoad;
    older.resolve([{ id: "review-old", kind: "ingest", summary: "Accepted input" }]);
    await oldLoad;

    expect(state.items).toEqual([arriving]);
  });

  it("invalidates an authenticated response when the lane is cleared", async () => {
    const pending = deferred<ReviewItem[]>();
    fake.responses = [pending.promise];
    const state = new ReviewState();

    const load = state.load();
    state.clear();
    pending.resolve([{ id: "private-review", kind: "ingest", summary: "Private" }]);
    await load;

    expect(state.items).toEqual([]);
  });
});
