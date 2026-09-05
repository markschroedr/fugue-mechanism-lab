<script lang="ts">
  import ArrowUp from "lucide-svelte/icons/arrow-up";
  import Mic from "lucide-svelte/icons/mic";
  import Plus from "lucide-svelte/icons/plus";
  import X from "lucide-svelte/icons/x";
  import { tick } from "svelte";
  import { session } from "../lib/session.svelte";
  import { review } from "../lib/review.svelte";
  import { workspace } from "../lib/workspace.svelte";
  import { shorten } from "../lib/text";
  import type { InputKind } from "../runtime/types";

  let textareaElement = $state<HTMLTextAreaElement | null>(null);
  let draft = $state("");
  let sending = $state(false);
  let error = $state<string | null>(null);
  let inputKind = $state<Exclude<InputKind, "comment">>("text");

  export function focus() {
    textareaElement?.focus();
  }

  // A session opened elsewhere (e.g. from a text selection) invites words next.
  $effect(() => {
    if (session.active && session.turns.length === 0) textareaElement?.focus();
  });

  $effect(() => {
    if (workspace.inputAnchor) textareaElement?.focus();
  });

  const placeholder = $derived(
    workspace.inputAnchor
      ? `Comment on “${shorten(workspace.inputAnchor.quote)}”`
      : session.active
      ? `Continue the session @ “${session.anchor?.title}”`
      : inputKind === "document"
        ? `Paste a document into “${workspace.activeSpace?.title ?? "this space"}”`
        : `Add anything to “${workspace.activeSpace?.title ?? "this space"}”`
  );

  const sendable = $derived(draft.trim().length > 0);

  function autogrow() {
    const el = textareaElement;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 176)}px`;
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      send();
    }
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    send();
  }

  async function send() {
    if (sending) return;
    const text = draft.trim();
    if (!text) return;

    sending = true;
    error = null;
    try {
      if (session.active) {
        await session.say(text);
      } else {
        const receipt = await workspace.submitInput(workspace.inputAnchor ? "comment" : inputKind, text);
        void review.waitForSource(receipt.artifactId, () => workspace.refreshSources());
      }
      draft = "";
      tick().then(autogrow);
    } catch (caught) {
      error = caught instanceof Error ? caught.message : "Failed to send";
    } finally {
      sending = false;
    }
  }
</script>

<div class="composer-shell">
  {#if error}
    <p class="inline-error composer-error" role="status">{error}</p>
  {/if}

  <form class="composer" onsubmit={submit}>
    <button
      class="icon"
      type="button"
      disabled={!workspace.inputAnchor}
      title={workspace.inputAnchor ? "Cancel comment" : "Add input"}
      aria-label={workspace.inputAnchor ? "Cancel comment" : "Add input"}
      onclick={() => workspace.cancelComment()}
    >
      {#if workspace.inputAnchor}
        <X size={18} strokeWidth={1.6} aria-hidden="true" />
      {:else}
        <Plus size={18} strokeWidth={1.6} aria-hidden="true" />
      {/if}
    </button>

    {#if !session.active && !workspace.inputAnchor}
      <select bind:value={inputKind} aria-label="Input kind">
        <option value="text">Text</option>
        <option value="document">Document</option>
      </select>
    {/if}

    <textarea
      bind:this={textareaElement}
      bind:value={draft}
      {placeholder}
      rows="1"
      aria-label="Composer"
      oninput={autogrow}
      onkeydown={handleKeydown}
    ></textarea>

    <button
      class="icon"
      type="button"
      disabled
      title="Voice input — not wired yet"
      aria-label="Voice input"
    >
      <Mic size={18} strokeWidth={1.6} aria-hidden="true" />
    </button>

    <button class="send" type="submit" disabled={!sendable || sending} aria-label="Send">
      <ArrowUp size={18} strokeWidth={1.6} aria-hidden="true" />
    </button>
  </form>
</div>

<style>
  .composer-shell {
    position: absolute;
    bottom: 1.25rem;
    left: 50%;
    z-index: 15;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    width: min(calc(100% - 2.5rem), 42rem);
    transform: translateX(-50%);
  }

  .composer {
    display: flex;
    align-items: flex-end;
    gap: 0.2rem;
    padding: 0.4rem;
    border: 1px solid var(--border-strong);
    border-radius: 26px;
    background: var(--panel-2);
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5);
  }

  .composer-error {
    padding: 0 0.7rem;
  }

  textarea {
    flex: 1;
    min-width: 0;
    min-height: 34px;
    max-height: 176px;
    padding: 0.42rem 0.4rem;
    border: 0;
    background: transparent;
    color: var(--text);
    line-height: 1.45;
    outline: none;
    resize: none;
  }

  select {
    align-self: center;
    width: 4.8rem;
    min-height: 30px;
    padding: 0 0.15rem;
    border: 0;
    background: transparent;
    color: var(--subtle);
    font-size: 0.72rem;
    font-weight: 650;
    outline: none;
  }

  textarea::placeholder {
    color: var(--subtle);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .icon,
  .send {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 34px;
    height: 34px;
    min-height: 0;
    padding: 0;
    border: 0;
    border-radius: 999px;
    background: none;
  }

  .icon {
    color: var(--muted);
  }

  .icon:disabled {
    color: var(--subtle);
    opacity: 1;
  }

  .send {
    background: var(--text);
    color: var(--bg);
    transition: background 120ms ease, color 120ms ease;
  }

  .send:disabled {
    background: transparent;
    color: var(--subtle);
    opacity: 1;
  }

</style>
