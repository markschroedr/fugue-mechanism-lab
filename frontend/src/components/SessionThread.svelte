<script lang="ts">
  import { session } from "../lib/session.svelte";
  import { review } from "../lib/review.svelte";

  let { variant }: { variant: "inline" | "dock" | "full" } = $props();
  let busy = $state<"commit" | "discard" | null>(null);

  async function commit() {
    if (busy) return;
    busy = "commit";
    try {
      const closed = await session.close();
      if (closed && (closed.turns.length > 0 || closed.stance)) {
        await review.load();
      }
    } catch {
      // session.error is rendered inline below.
    } finally {
      busy = null;
    }
  }

  async function discard() {
    if (busy) return;
    busy = "discard";
    try {
      await session.discard();
    } catch {
      // session.error is rendered inline below.
    } finally {
      busy = null;
    }
  }
</script>

<section class="session session--{variant}" aria-label="Active session">
  <header class="head">
    <span class="label">
      Session · private
      {#if session.anchor && !session.anchor.quote}
        <span class="anchor">@ “{session.anchor.title}”</span>
      {/if}
      {#if session.stance}
        <span class="stance">{session.stance}</span>
      {/if}
    </span>
    <span class="actions">
      <button class="quiet" type="button" disabled={busy !== null} onclick={discard}>Discard</button>
      <button class="commit" type="button" disabled={!session.committable || busy !== null} onclick={commit}>
        Commit
      </button>
    </span>
  </header>

  {#if session.error}
    <p class="inline-error" role="status">{session.error}</p>
  {/if}

  {#if session.anchor?.quote && variant !== "inline"}
    <blockquote class="quote">“{session.anchor.quote}”</blockquote>
  {/if}

  <ol class="turns">
    {#each session.turns as turn (turn.id)}
      <li class="turn">
        <span class="who">{turn.who}</span>
        <p class="text">{turn.text}</p>
      </li>
    {/each}
  </ol>
</section>

<style>
  .session {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
    padding: 0.9rem 1rem;
    border: 1px dashed var(--accent-dim);
    border-radius: 8px;
    background: var(--panel);
  }

  .session--inline {
    margin: 1.2rem 0 1.2rem 1.1rem;
  }

  .session--dock,
  .session--full {
    height: 100%;
    padding: 1.25rem 1.1rem;
    border: 0;
    border-radius: 0;
    background: transparent;
    overflow-y: auto;
  }

  /* Keep the last turns clear of the floating composer. */
  .session--full {
    padding-bottom: 6rem;
  }

  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.4rem 0.5rem;
  }

  .label {
    display: flex;
    align-items: baseline;
    gap: 0.3rem;
    min-width: 0;
    color: var(--accent-dim);
    font-size: 0.68rem;
    font-weight: 700;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    white-space: nowrap;
  }

  .anchor {
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--muted);
    font-weight: 500;
    letter-spacing: 0.02em;
    text-transform: none;
  }

  .stance {
    flex-shrink: 0;
    padding: 0.06rem 0.45rem;
    border: 1px solid var(--accent-dim);
    border-radius: 999px;
    color: var(--accent);
  }

  .actions {
    display: flex;
    gap: 0.3rem;
    flex-shrink: 0;
  }

  .quote {
    margin: 0;
    padding-left: 0.75rem;
    border-left: 2px solid var(--accent-dim);
    color: var(--muted);
    font-family: var(--font-doc);
    font-size: 0.92rem;
    font-style: italic;
    line-height: 1.5;
  }

  .commit {
    min-height: 0;
    padding: 0.24rem 0.7rem;
    border-radius: 5px;
    font-size: 0.78rem;
    font-weight: 650;
  }

  .turns {
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .turn {
    display: flex;
    gap: 0.6rem;
  }

  .who {
    min-width: 2.6rem;
    padding-top: 0.1rem;
    color: var(--subtle);
    font-family: var(--font-mono);
    font-size: 0.68rem;
    text-transform: uppercase;
  }

  .text {
    flex: 1;
    margin: 0;
    font-size: 0.88rem;
    line-height: 1.5;
  }
</style>
