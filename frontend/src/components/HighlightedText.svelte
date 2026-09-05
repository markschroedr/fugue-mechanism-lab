<script lang="ts">
  import { session } from "../lib/session.svelte";

  let { blockId, text }: { blockId: string; text: string } = $props();

  /** While a session is anchored to a span of this block, that span stays marked. */
  const parts = $derived.by(() => {
    const anchor = session.anchor;
    if (!anchor || anchor.blockId !== blockId || !anchor.quote) return null;
    const index = text.indexOf(anchor.quote);
    if (index === -1) return null;
    return {
      before: text.slice(0, index),
      quote: anchor.quote,
      after: text.slice(index + anchor.quote.length)
    };
  });
</script>

{#if parts}{parts.before}<mark>{parts.quote}</mark>{parts.after}{:else}{text}{/if}
