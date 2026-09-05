<script lang="ts">
  import { review } from "../lib/review.svelte";
  import { workspace } from "../lib/workspace.svelte";
  import type { ReviewItem } from "../lib/types";

  const kindLabels: Record<ReviewItem["kind"], string> = {
    session: "Your session",
    entity: "Entity proposal",
    ingest: "Agent ingest",
    objection: "Public objection",
    lint: "Lint"
  };
  let acceptingId = $state<string | null>(null);

  async function reject(id: string) {
    if (acceptingId) return;
    acceptingId = id;
    try {
      await review.reject(id);
    } catch {
      // review.error is rendered inline below.
    } finally {
      acceptingId = null;
    }
  }

  async function accept(id: string) {
    if (acceptingId) return;
    acceptingId = id;
    const previousVersion = workspace.report?.version ?? 0;
    try {
      const resolved = await review.resolve(id);
      if (!resolved) return;
      workspace.replaceDoc(resolved.doc);
      void workspace.waitForReportAfter(previousVersion, resolved.doc.spaceId);
    } catch {
      // review.error is rendered inline below.
    } finally {
      acceptingId = null;
    }
  }
</script>

<section class="lane" aria-label="Review lane">
  <h2>Review</h2>
  {#if review.error}
    <p class="inline-error" role="status">{review.error}</p>
  {/if}
  <ol class="items">
    {#each review.items as item (item.id)}
      <li class="item">
        <span class="kind">{kindLabels[item.kind]}</span>
        <p class="summary">{item.summary}</p>
        {#if item.unverifiedDisplayName}
          <p class="identity">{item.unverifiedDisplayName} · unverified</p>
        {/if}
        {#if item.objectionAnchor}
          <blockquote>Report v{item.objectionAnchor.reportVersion}, characters {item.objectionAnchor.start}–{item.objectionAnchor.end}: “{item.objectionAnchor.quote}”</blockquote>
        {/if}
        {#if item.detail}
          <p class="detail">{item.detail}</p>
        {/if}
        <div class="actions">
          <button class="accept" type="button" disabled={acceptingId !== null} onclick={() => accept(item.id)}>
            {item.fromSession ? "Commit" : "Accept"}
          </button>
          <button class="reject" type="button" disabled={acceptingId !== null} onclick={() => reject(item.id)}>Reject</button>
        </div>
      </li>
    {:else}
      <li class="item item--empty">
        <p class="detail">Nothing waiting on you.</p>
      </li>
    {/each}
  </ol>
</section>

<style>
  .lane {
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
    height: 100%;
    overflow-y: auto;
    /* Extra bottom room so the floating composer never covers the last item. */
    padding: 1.25rem 1.1rem 6rem;
  }

  h2 {
    margin: 0;
    color: var(--subtle);
    font-size: 0.68rem;
    font-weight: 700;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }

  .items {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .item {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.7rem 0.8rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--panel);
  }

  .item--empty {
    border-style: dashed;
    background: transparent;
  }

  .kind {
    color: var(--subtle);
    font-size: 0.66rem;
    font-weight: 700;
    letter-spacing: 0.09em;
    text-transform: uppercase;
  }

  .summary {
    margin: 0;
    font-size: 0.85rem;
    line-height: 1.45;
  }

  .detail,
  .identity,
  blockquote {
    margin: 0;
    color: var(--muted);
    font-size: 0.78rem;
    line-height: 1.45;
  }

  .identity {
    color: var(--subtle);
    font-size: 0.7rem;
  }

  blockquote {
    padding-left: 0.6rem;
    border-left: 2px solid var(--border-strong);
    color: var(--subtle);
  }

  .actions {
    display: flex;
    gap: 0.35rem;
    margin-top: 0.2rem;
  }

  .accept, .reject {
    min-height: 0;
    padding: 0.24rem 0.7rem;
    border-radius: 5px;
    font-size: 0.78rem;
    font-weight: 650;
  }

  .reject { border-color: transparent; background: transparent; color: var(--muted); }
</style>
