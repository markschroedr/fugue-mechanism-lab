import { runtime } from "../runtime";
import type { ReviewItem, SynthesisDoc } from "./types";

/**
 * The review lane: every state-changing proposal waits here for consent (I9).
 * It hides while a session is live and reappears once the session commits.
 */
export class ReviewState {
  items = $state<ReviewItem[]>([]);
  error = $state<string | null>(null);
  private loadGeneration = 0;

  clear() {
    this.loadGeneration += 1;
    this.items = [];
    this.error = null;
  }

  async load() {
    const generation = ++this.loadGeneration;
    try {
      const items = await runtime.listReview();
      if (generation !== this.loadGeneration) return;
      this.items = items;
      this.error = null;
    } catch (error) {
      if (generation !== this.loadGeneration) return;
      this.error = error instanceof Error ? error.message : "Failed to load review lane";
    }
  }

  async waitForSource(sourceArtifactId: string, onRefresh?: () => Promise<void>) {
    const startedAt = Date.now();
    while (Date.now() - startedAt < 240_000) {
      await this.load();
      try {
        await onRefresh?.();
      } catch {
        // The next polling turn retries transient source-read failures.
      }
      if (this.items.some((item) => item.sourceArtifactId === sourceArtifactId)) return;
      await new Promise((resolve) => window.setTimeout(resolve, 1500));
    }
  }

  async reject(id: string): Promise<void> {
    try {
      this.error = null;
      await runtime.rejectReviewItem(id);
      await this.load();
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to reject review item";
      throw error;
    }
  }

  async resolve(id: string): Promise<{ item: ReviewItem; doc: SynthesisDoc } | undefined> {
    try {
      this.error = null;
      const item = this.items.find((candidate) => candidate.id === id);
      const doc = await runtime.acceptReviewItem(id);
      await this.load();
      return item ? { item, doc } : undefined;
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to accept review item";
      throw error;
    }
  }
}

export const review = new ReviewState();
