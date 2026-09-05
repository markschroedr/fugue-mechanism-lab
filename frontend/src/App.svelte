<script lang="ts">
  import { Tabs } from "bits-ui";
  import { onMount } from "svelte";
  import AuthPanel from "./components/AuthPanel.svelte";
  import Composer from "./components/Composer.svelte";
  import LivingDocument from "./components/LivingDocument.svelte";
  import ProjectionRail from "./components/ProjectionRail.svelte";
  import ReviewLane from "./components/ReviewLane.svelte";
  import SessionThread from "./components/SessionThread.svelte";
  import TopBar from "./components/TopBar.svelte";
  import { auth } from "./lib/auth.svelte";
  import { review } from "./lib/review.svelte";
  import { session } from "./lib/session.svelte";
  import { workspace } from "./lib/workspace.svelte";

  let innerWidth = $state(1200);
  let composer = $state<Composer | null>(null);
  let railOpen = $state(false);
  let mobileTab = $state<"document" | "session" | "review">("document");

  const isMobile = $derived(innerWidth < 760);

  /**
   * The right flank belongs to exactly one thing at a time: the review lane
   * by default; nothing while a session runs inline (reading width wins);
   * the docked session once its anchor scrolls out of view.
   */
  const flank = $derived(
    auth.actor ? (session.active ? (session.anchorInView ? "hidden" : "session") : "review") : "hidden"
  );

  $effect(() => {
    if (!session.active && mobileTab === "session") mobileTab = "document";
  });

  const REVIEW_POLL_INTERVAL_MS = 10_000;

  onMount(() => {
    // Bootstrap reads are one-shot. Keeping workspace.load() outside an effect is
    // important because it synchronously reads and writes its reactive loading flag.
    void auth.load();
    void workspace.load();
  });

  $effect(() => {
    if (!auth.actor) {
      review.clear();
      return;
    }
    void review.load();
    // Worker-created review items have no push channel yet; poll deliberately and
    // slowly, while explicit user flows refresh the lane immediately.
    const timer = window.setInterval(() => void review.load(), REVIEW_POLL_INTERVAL_MS);
    return () => window.clearInterval(timer);
  });

  $effect(() => {
    if (!session.active) return;
    const timer = window.setInterval(() => {
      void session.refresh();
    }, 1200);
    return () => window.clearInterval(timer);
  });

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "k" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      composer?.focus();
    }
  }
</script>

<svelte:window bind:innerWidth onkeydown={handleKeydown} />

<Tabs.Root class="shell" bind:value={mobileTab}>
  <TopBar onToggleRail={() => (railOpen = !railOpen)} />

  <div class="body" class:flank-hidden={!isMobile && flank === "hidden"}>
    <aside class="rail-slot" class:open={railOpen}>
      <ProjectionRail onNavigate={() => (railOpen = false)} />
    </aside>
    {#if railOpen && isMobile}
      <button class="scrim" type="button" aria-label="Close spaces" onclick={() => (railOpen = false)}></button>
    {/if}

    <main class="center">
      <div class="scroll">
        {#if !isMobile}
          <LivingDocument />
        {:else}
          <Tabs.Content value="document" class="mobile-pane">
            <LivingDocument />
          </Tabs.Content>
          {#if auth.actor}
            <Tabs.Content value="session" class="mobile-pane">
              {#if session.active}
                <SessionThread variant="full" />
              {:else}
                <p class="pane-empty">No open session. Start one from the composer.</p>
              {/if}
            </Tabs.Content>
            <Tabs.Content value="review" class="mobile-pane">
              <ReviewLane />
            </Tabs.Content>
          {/if}
        {/if}
      </div>
      {#if auth.actor}
        <Composer bind:this={composer} />
      {:else if !auth.loading}
        <AuthPanel />
      {/if}
    </main>

    {#if !isMobile && flank !== "hidden"}
      <aside class="flank">
        {#if flank === "session"}
          <SessionThread variant="dock" />
        {:else}
          <ReviewLane />
        {/if}
      </aside>
    {/if}
  </div>

  {#if isMobile}
    <Tabs.List class="mobile-tabs" aria-label="Panes">
      <Tabs.Trigger value="document">
        Document
      </Tabs.Trigger>
      {#if auth.actor}
        <Tabs.Trigger value="session" disabled={!session.active}>
          Session{session.active ? " ·" : ""}
        </Tabs.Trigger>
      {/if}
      {#if auth.actor}
        <Tabs.Trigger value="review">
          Review{review.items.length > 0 ? ` · ${review.items.length}` : ""}
        </Tabs.Trigger>
      {/if}
    </Tabs.List>
  {/if}
</Tabs.Root>

<style>
  :global(.shell) {
    display: flex;
    flex-direction: column;
    height: 100vh;
    height: 100dvh;
  }

  .body {
    position: relative;
    display: grid;
    flex: 1;
    grid-template-columns: 232px minmax(0, 1fr) 320px;
    min-height: 0;
  }

  .body.flank-hidden {
    grid-template-columns: 232px minmax(0, 1fr);
  }

  .rail-slot {
    min-height: 0;
  }

  .center {
    position: relative;
    min-height: 0;
  }

  .scroll {
    height: 100%;
    overflow-y: auto;
  }

  :global(.mobile-pane) {
    min-height: 100%;
  }

  .flank {
    min-height: 0;
    overflow: hidden;
    border-left: 1px solid var(--border);
  }

  .pane-empty {
    padding: 2rem 1.2rem;
    color: var(--subtle);
  }

  :global(.mobile-tabs) {
    display: flex;
    border-top: 1px solid var(--border);
    background: var(--panel);
  }

  :global(.mobile-tabs) :global(button) {
    flex: 1;
    min-height: 0;
    padding: 0.55rem 0;
    border: 0;
    border-top: 2px solid transparent;
    border-radius: 0;
    background: none;
    color: var(--muted);
    cursor: pointer;
    font-size: 0.78rem;
    font-weight: 600;
  }

  :global(.mobile-tabs) :global(button[data-state="active"]) {
    border-top-color: var(--text);
    color: var(--text);
  }

  :global(.mobile-tabs) :global(button:disabled) {
    color: var(--subtle);
    cursor: default;
    opacity: 0.6;
  }

  .scrim {
    position: absolute;
    inset: 0;
    z-index: 5;
    min-height: 0;
    border: 0;
    border-radius: 0;
    background: rgba(0, 0, 0, 0.55);
    cursor: default;
  }

  @media (max-width: 760px) {
    .body {
      grid-template-columns: minmax(0, 1fr);
    }

    .rail-slot {
      position: absolute;
      inset: 0 30% 0 0;
      z-index: 10;
      transform: translateX(-100%);
      transition: transform 160ms ease;
    }

    .rail-slot.open {
      transform: translateX(0);
    }
  }
</style>
