<script lang="ts">
  import Plus from "lucide-svelte/icons/plus";
  import Waypoints from "lucide-svelte/icons/waypoints";
  import { auth } from "../lib/auth.svelte";
  import { workspace } from "../lib/workspace.svelte";
  import type { Projection } from "../lib/types";

  let { onNavigate }: { onNavigate?: () => void } = $props();

  const projections: Array<{ key: Projection; label: string }> = [
    { key: "belief", label: "Belief" },
    { key: "desire", label: "Desire" },
    { key: "action", label: "Action" }
  ];
  let creatingProjection = $state<Projection | null>(null);
  let draftTitle = $state("");
  let busy = $state(false);

  function select(id: string) {
    workspace.selectSpace(id);
    onNavigate?.();
  }

  async function create(projection: Projection) {
    if (busy) return;
    const title = draftTitle.trim();
    if (!title) return;
    busy = true;
    try {
      await workspace.createSpace(title, projection);
      draftTitle = "";
      creatingProjection = null;
      onNavigate?.();
    } catch {
      // workspace.error is rendered by the document surface.
    } finally {
      busy = false;
    }
  }
</script>

<nav class="rail" aria-label="Spaces by projection">
  {#each projections as projection (projection.key)}
    <section class="group">
      <header class="group-head">
        <h2>{projection.label}</h2>
        {#if auth.actor}
          <button
            class="add"
            type="button"
            aria-label={`Create ${projection.label} space`}
            title={`Create ${projection.label} space`}
            onclick={() => {
              creatingProjection = creatingProjection === projection.key ? null : projection.key;
              draftTitle = "";
            }}
          >
            <Plus size={16} strokeWidth={1.6} aria-hidden="true" />
          </button>
        {/if}
      </header>
      {#if creatingProjection === projection.key}
        <form class="new-space" onsubmit={(event) => {
          event.preventDefault();
          void create(projection.key);
        }}>
          <input
            bind:value={draftTitle}
            placeholder="Space title"
            aria-label={`${projection.label} space title`}
            disabled={busy}
            required
          />
          <button type="submit" disabled={busy || !draftTitle.trim()}>Create</button>
        </form>
      {/if}
      <ul>
        {#each workspace.spaces.filter((space) => space.projection === projection.key) as space (space.id)}
          {@const isEcho = space.id !== workspace.echo.sourceId && workspace.echo.neighborIds.has(space.id)}
          <li>
            <button
              type="button"
              class:active={space.id === workspace.activeSpaceId}
              class:echo={isEcho}
              class:echo--hover={isEcho && workspace.echo.fromHover}
              onclick={() => select(space.id)}
              onmouseenter={() => (workspace.hoveredSpaceId = space.id)}
              onmouseleave={() => (workspace.hoveredSpaceId = null)}
              onfocus={() => (workspace.hoveredSpaceId = space.id)}
              onblur={() => (workspace.hoveredSpaceId = null)}
            >
              <span class="title">{space.title}</span>
              {#if isEcho}
                <span class="tie" aria-hidden="true">
                  <Waypoints size={13} strokeWidth={1.6} />
                </span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    </section>
  {/each}
</nav>

<style>
  .rail {
    display: flex;
    flex-direction: column;
    gap: 1.6rem;
    height: 100%;
    overflow-y: auto;
    padding: 1.25rem 0.75rem;
    border-right: 1px solid var(--border);
    background: var(--panel);
  }

  .group-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.4rem;
    color: var(--subtle);
    margin: 0 0 0.4rem;
    padding: 0 0.35rem 0 0.55rem;
  }

  .group h2 {
    margin: 0;
    font-size: 0.68rem;
    font-weight: 700;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }

  .add {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    min-height: 0;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--subtle);
    font-size: 1rem;
    line-height: 1;
  }

  .add:hover:not(:disabled) {
    background: var(--hover);
    color: var(--text);
  }

  .new-space {
    display: flex;
    gap: 0.25rem;
    margin: 0 0 0.45rem;
    padding: 0 0.3rem;
  }

  .new-space input {
    min-width: 0;
    width: 100%;
    min-height: 30px;
    padding: 0.28rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--input-bg);
    color: var(--text);
    font-size: 0.84rem;
    outline: none;
  }

  .new-space button {
    min-height: 30px;
    padding: 0.28rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--hover);
    color: var(--muted);
    font-size: 0.74rem;
  }

  ul {
    display: flex;
    flex-direction: column;
    gap: 2px;
    list-style: none;
    margin: 0;
    padding: 0;
  }

  li > button {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.4rem;
    width: 100%;
    min-height: 0;
    padding: 0.34rem 0.55rem;
    border: 1px solid transparent;
    border-radius: 5px;
    background: none;
    color: var(--muted);
    cursor: pointer;
    font-size: 0.88rem;
    font-weight: 500;
    text-align: left;
    transition: color 120ms ease, border-color 120ms ease;
  }

  li > button:hover {
    background: var(--hover);
    color: var(--text);
  }

  li > button.active {
    background: var(--active-bg);
    border-color: var(--border);
    color: var(--text);
    font-weight: 650;
  }

  /* The cross-projection echo: nearest neighbors of the active space stay
     faintly marked; hovering a space lights its own neighbors up stronger. */
  li > button.echo {
    border-color: var(--border);
    border-style: dashed;
    color: var(--text);
  }

  li > button.echo--hover {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .tie {
    color: var(--subtle);
    font-size: 0.8rem;
  }

  .echo--hover .tie {
    color: var(--accent);
  }

  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
