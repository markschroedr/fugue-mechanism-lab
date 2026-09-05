<script lang="ts">
  import HighlightedText from "./HighlightedText.svelte";
  import HolderAttribution from "./HolderAttribution.svelte";
  import { auth } from "../lib/auth.svelte";
  import { workspace } from "../lib/workspace.svelte";
  import type { DocNode } from "../lib/types";

  type EntityNode = Extract<DocNode, { kind: "entity" }>;

  let {
    node,
    focused,
    onFocusBlock
  }: {
    node: EntityNode;
    focused: boolean;
    onFocusBlock: (id: string) => void;
  } = $props();

  const posteriorLabel = $derived(
    node.posterior == null ? null : node.posterior.toFixed(2)
  );
  let editing = $state(false);
  let draft = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);

  $effect(() => {
    if (!editing) draft = node.text;
  });

  async function saveEdit(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    const text = draft.trim();
    if (!text || text === node.text) {
      editing = false;
      draft = node.text;
      return;
    }
    busy = true;
    error = null;
    try {
      await workspace.editEntityText(node.id, text);
      editing = false;
    } catch (caught) {
      error = caught instanceof Error ? caught.message : "Failed to edit entity";
    } finally {
      busy = false;
    }
  }

  async function toggleIssueState() {
    if (busy || node.role !== "issue") return;
    busy = true;
    error = null;
    try {
      await workspace.changeEntityState(node.id, node.state === "resolved" ? "open" : "resolved");
    } catch (caught) {
      error = caught instanceof Error ? caught.message : "Failed to update entity state";
    } finally {
      busy = false;
    }
  }
</script>

<div
  class="entity"
  class:dissent={node.dissent}
  class:focused
  onclick={() => onFocusBlock(node.id)}
  onkeydown={(event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onFocusBlock(node.id);
    }
  }}
  tabindex="0"
  role="button"
  aria-pressed={focused}
>
  <header class="meta">
    <span class="role">{node.role}</span>
    <span class="state">· {node.state}</span>
    {#if posteriorLabel}
      <span class="state">· {posteriorLabel}</span>
    {/if}
    {#if node.halfLifeDays != null}
      <span class="halflife">decays in {node.halfLifeDays}d</span>
    {/if}
  </header>

  <div class="content" data-block-id={node.id}>
    {#if editing}
      <form class="edit-form" onsubmit={saveEdit}>
        <textarea bind:value={draft} rows="3" aria-label="Entity text" disabled={busy}></textarea>
        <span class="edit-actions">
          <button class="quiet" type="button" disabled={busy} onclick={() => {
            editing = false;
            draft = node.text;
          }}>Cancel</button>
          <button class="quiet" type="submit" disabled={busy || !draft.trim()}>Save</button>
        </span>
      </form>
    {:else}
      <HighlightedText blockId={node.id} text={node.text} />
    {/if}
  </div>

  {#if error}
    <p class="inline-error" role="status">{error}</p>
  {/if}

  {#if auth.actor}
    <div class="entity-actions" aria-label="Entity actions">
      <button class="quiet" type="button" disabled={busy} onclick={() => (editing = true)}>Edit</button>
      {#if node.role === "issue"}
        <button class="quiet" type="button" disabled={busy} onclick={toggleIssueState}>
          {node.state === "resolved" ? "Reopen" : "Resolve"}
        </button>
      {/if}
    </div>
  {/if}

  {#if node.holders.length > 0}
    <div class="holders" aria-label="Position holders">
      {#each node.holders as holder (holder.actorId + holder.stance)}
        <HolderAttribution {holder} targetEntityId={node.id} previewSource={(artifactId) => void workspace.previewSource(artifactId)} />
      {/each}
    </div>
  {/if}

  {#if node.posterior != null}
    <div
      class="posterior"
      role="meter"
      aria-valuenow={node.posterior}
      aria-valuemin="0"
      aria-valuemax="1"
      aria-label="Cached posterior"
    >
      <i style:width="{node.posterior * 100}%"></i>
    </div>
  {/if}

  {#if node.markets?.length}
    <p class="markets">
      {#each node.markets as market, index (market.name)}
        {index > 0 ? " · " : ""}{market.name} {market.value.toFixed(2)}
      {/each}
    </p>
  {/if}

  {#if node.neighbors.length > 0}
    <footer class="resonance">
      {#each node.neighbors as neighbor (neighbor.projection + neighbor.title)}
        <span class="chip">
          <span class="chip-projection">{neighbor.projection}</span>
          {neighbor.title}
        </span>
      {/each}
    </footer>
  {/if}
</div>

<style>
  .entity {
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
    margin: 1.2rem 0;
    padding: 0.2rem 0 0.2rem 1.1rem;
    border-left: 2px solid var(--border-strong);
    cursor: pointer;
    outline: none;
    transition: border-color 120ms ease;
  }

  .entity:hover,
  .entity:focus-visible {
    border-left-color: var(--muted);
  }

  .entity.focused {
    border-left-color: var(--accent);
  }

  .entity.dissent {
    border-left-style: dashed;
  }

  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.35rem;
    color: var(--subtle);
    font-size: 0.7rem;
    font-weight: 700;
    letter-spacing: 0.09em;
    text-transform: uppercase;
  }

  .halflife {
    margin-left: auto;
    color: var(--subtle);
    font-weight: 500;
    letter-spacing: 0.02em;
    text-transform: none;
  }

  .content {
    margin: 0;
    color: var(--text);
    font-family: var(--font-doc);
    font-size: 1.05rem;
    line-height: 1.6;
  }

  .edit-form {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
  }

  .edit-form textarea {
    width: 100%;
    min-height: 6rem;
    padding: 0.55rem 0.65rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--input-bg);
    color: var(--text);
    font-family: var(--font-doc);
    line-height: 1.55;
    outline: none;
    resize: vertical;
  }

  .edit-actions,
  .entity-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }

  .entity-actions {
    margin-top: -0.15rem;
  }

  .holders {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem 0.8rem;
    color: var(--muted);
    font-size: 0.76rem;
  }

  .posterior {
    position: relative;
    width: min(120px, 100%);
    height: 3px;
    border-radius: 2px;
    background: var(--panel-2);
    overflow: hidden;
  }

  .posterior i {
    position: absolute;
    inset: 0 auto 0 0;
    border-radius: 2px;
    background: var(--accent-dim);
  }

  .markets {
    margin: 0;
    color: var(--subtle);
    font-size: 0.76rem;
  }

  .resonance {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.35rem;
    margin-top: 0.1rem;
  }

  .chip {
    display: inline-flex;
    align-items: baseline;
    gap: 0.3rem;
    padding: 0.12rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--muted);
    font-size: 0.74rem;
  }

  .chip-projection {
    color: var(--accent-dim);
    font-size: 0.64rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
</style>
