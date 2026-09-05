<script lang="ts">
  import Menu from "lucide-svelte/icons/menu";
  import { runtime, type CostSummary, type HealthResponse } from "../runtime";
  import { auth } from "../lib/auth.svelte";
  import { review } from "../lib/review.svelte";

  let { onToggleRail }: { onToggleRail: () => void } = $props();

  let health = $state<HealthResponse | null>(null);
  let healthError = $state(false);
  let costs = $state<CostSummary | null>(null);

  $effect(() => {
    runtime
      .health()
      .then((response) => {
        health = response;
        healthError = false;
      })
      .catch(() => {
        healthError = true;
      });
    const loadCosts = () => runtime.readCosts().then((value) => (costs = value)).catch(() => undefined);
    void loadCosts();
    const timer = window.setInterval(loadCosts, 10_000);
    return () => window.clearInterval(timer);
  });
</script>

<header class="topbar">
  <button class="rail-toggle" type="button" onclick={onToggleRail} aria-label="Toggle spaces">
    <Menu size={18} strokeWidth={1.6} aria-hidden="true" />
  </button>
  <span class="brand">Fugue</span>
  <span class="spacer"></span>
  {#if auth.actor}
    <span class="identity">{auth.actor.username}</span>
    <button class="logout" type="button" onclick={() => auth.logout()}>Log out</button>
    {#if review.items.length > 0}
      <span class="pill review-badge">Review · {review.items.length}</span>
    {/if}
  {/if}
  {#if costs}
    <span class="cost" title={`${costs.overall.inputTokens + costs.overall.outputTokens} tokens · ${costs.servedModels.join(", ") || health?.model || "no calls"}`}>
      ${costs.overall.costUsd.toFixed(4)}
    </span>
  {/if}
  <span
    class="status-dot"
    class:status-dot--ok={health !== null && !healthError}
    title={healthError ? "Backend unreachable" : (health?.status ?? "Connecting")}
  ></span>
</header>

<style>
  .topbar {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    min-height: 48px;
    padding: 0 1.25rem;
    border-bottom: 1px solid var(--border);
  }

  .brand {
    font-family: var(--font-doc);
    font-size: 1.05rem;
    font-style: italic;
    font-weight: 600;
    letter-spacing: 0.01em;
  }

  .spacer {
    flex: 1;
  }

  .rail-toggle {
    display: none;
    border: 0;
    background: none;
    color: var(--muted);
    cursor: pointer;
    font-size: 1rem;
    padding: 0.25rem 0.4rem;
    min-height: 0;
  }

  .review-badge {
    display: none;
  }

  .identity, .logout, .cost { color: var(--subtle); font-size: .72rem; }
  .cost { font-variant-numeric: tabular-nums; }
  .logout { min-height: 0; padding: .2rem .35rem; border: 0; background: transparent; }

  .status-dot {
    width: 0.4rem;
    height: 0.4rem;
    border-radius: 999px;
    background: var(--error);
    flex-shrink: 0;
  }

  .status-dot--ok {
    background: var(--ok);
  }

  @media (max-width: 760px) {
    .rail-toggle {
      display: block;
    }

    .review-badge {
      display: inline-block;
    }
  }
</style>
