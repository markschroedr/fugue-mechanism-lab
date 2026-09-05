<script lang="ts">
  import { auth } from "../lib/auth.svelte";
  import { review } from "../lib/review.svelte";
  import { workspace } from "../lib/workspace.svelte";
  import type { PositionHolder, SourceLink } from "../lib/types";

  let {
    holder,
    targetEntityId,
    previewSource
  }: {
    holder: PositionHolder;
    targetEntityId: string;
    previewSource: (artifactId: string) => void;
  } = $props();
  const history = $derived(holder.history ?? []);
  const conditions = $derived(holder.conditions ?? []);
  let conditionDraft = $state("");
  let declaring = $state(false);
  let declarationError = $state<string | null>(null);

  async function declareCondition(event: SubmitEvent) {
    event.preventDefault();
    event.stopPropagation();
    const terms = conditionDraft.trim();
    if (!terms || declaring) return;
    declaring = true;
    declarationError = null;
    try {
      await workspace.declareCondition(targetEntityId, terms);
      await review.load();
      conditionDraft = "";
    } catch (error) {
      declarationError = error instanceof Error ? error.message : "Failed to declare condition";
    } finally {
      declaring = false;
    }
  }

  function datePart(value: string | undefined) {
    return value?.slice(0, 10);
  }
</script>

<div class="attribution">
  <span class:disputes={holder.stance === "dispute"}>
    {holder.actorName} {holder.stance === "dispute" ? "disputes" : "holds"}
    {#if holder.statedConfidence}<span class="confidence">· {holder.statedConfidence}</span>{/if}
    {#each holder.evidence as source (source.artifactId)}
      {@render evidenceLink(source)}
    {/each}
    <span class="dates" title={`Accepted ${holder.acceptedAt}`}>
      {#if holder.evidence[0]?.authoredAt}authored {datePart(holder.evidence[0].authoredAt)} · {/if}accepted {datePart(holder.acceptedAt)}
    </span>
  </span>
  {#if conditions.length > 0}
    <section class="conditions" aria-label={`Conditions stated by ${holder.actorName}`}>
      <strong>Conditions</strong>
      {#each conditions as condition (condition.contractId)}
        <div>
          <span>{condition.conditionText}</span>
          {#if condition.terms !== condition.conditionText}<q>{condition.terms}</q>{/if}
          {@render evidenceLink(condition.evidence)}
        </div>
      {/each}
    </section>
  {/if}
  {#if auth.actor?.actorId === holder.actorId}
    <form class="declare" onsubmit={declareCondition}>
      <input bind:value={conditionDraft} placeholder="What would change your mind?" aria-label="Declare a condition on your stance" disabled={declaring} />
      <button type="submit" disabled={!conditionDraft.trim() || declaring}>{declaring ? "…" : "+"}</button>
    </form>
    {#if declarationError}<small class="error" role="status">{declarationError}</small>{/if}
  {/if}
  {#if history.length > 0}
    <details class="history">
      <summary>{history.length} prior</summary>
      {#each history as prior}
        <div>
          {prior.stance === "dispute" ? "disputed" : "held"}
          {#if prior.statedConfidence}<span class="confidence">· {prior.statedConfidence}</span>{/if}
          {#each prior.evidence as source (source.artifactId)}
            {@render evidenceLink(source)}
          {/each}
          <span class="dates" title={`Accepted ${prior.acceptedAt}`}>
            {#if prior.evidence[0]?.authoredAt}authored {datePart(prior.evidence[0].authoredAt)} · {/if}accepted {datePart(prior.acceptedAt)}
          </span>
        </div>
      {/each}
    </details>
  {/if}
</div>

{#snippet evidenceLink(source: SourceLink)}
  {#if source.url}
    <a href={source.url} target="_blank" rel="noreferrer" title={`Evidence: ${source.label}`} onclick={(event) => event.stopPropagation()}>source</a>
  {:else}
    <button class="source-link" type="button" title={`Evidence: ${source.label}`} onclick={(event) => {
      event.stopPropagation();
      previewSource(source.artifactId);
    }}>source</button>
  {/if}
{/snippet}

<style>
  .attribution { min-width: 0; }
  span.disputes { color: var(--subtle); }
  .confidence { color: var(--ink); }
  a, .source-link { margin-left: 0.2rem; color: var(--accent); font: inherit; }
  .source-link { padding: 0; border: 0; background: transparent; text-decoration: underline; cursor: pointer; }
  .dates { margin-left: 0.35rem; color: var(--subtle); font-size: 0.68rem; white-space: nowrap; }
  .conditions { margin: 0.3rem 0 0 0.75rem; color: var(--subtle); font-size: 0.72rem; }
  .conditions strong { display: block; color: var(--ink); font-weight: 600; }
  .conditions div { margin-top: 0.18rem; }
  .conditions q { display: block; margin-top: 0.1rem; font-style: italic; }
  .declare { display: flex; gap: 0.3rem; margin: 0.35rem 0 0 0.75rem; }
  .declare input { width: min(23rem, 100%); border: 0; border-bottom: 1px solid var(--border); background: transparent; color: var(--ink); font: inherit; }
  .declare button { border: 0; background: transparent; color: var(--accent); cursor: pointer; }
  .error { display: block; margin-left: 0.75rem; color: var(--danger, #d77); }
  .history { margin: 0.18rem 0 0 0.75rem; color: var(--subtle); font-size: 0.68rem; }
  .history summary { width: fit-content; cursor: pointer; }
  .history div { margin-top: 0.16rem; }
</style>
