<script lang="ts">
  import { session } from "../lib/session.svelte";
  import { auth } from "../lib/auth.svelte";
  import { workspace } from "../lib/workspace.svelte";
  import { resolveMarkdownRange, type MarkdownSourceRange } from "../lib/markdown-position";
  import { shorten } from "../lib/text";
  import type { Stance } from "../lib/types";

  type Target = {
    blockId: string;
    text: string;
    left: number;
    top: number;
    start: number;
    end: number;
    reportSource?: MarkdownSourceRange;
  };

  let target = $state<Target | null>(null);
  let opening = $state(false);
  let objectionOpen = $state(false);
  let objectionDraft = $state("");
  let displayName = $state("");
  let submitted = $state(false);
  let error = $state<string | null>(null);

  /**
   * A selection becomes a target when it lies inside a single document block's
   * text (marked with data-block-id). That span is what the session — and
   * ultimately the commit — refers to.
   */
  function readSelection(): Target | null {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;
    const text = selection.toString().trim();
    if (!text) return null;

    const range = selection.getRangeAt(0);
    const start = elementOf(range.startContainer)?.closest("[data-block-id]");
    const end = elementOf(range.endContainer)?.closest("[data-block-id]");
    if (!start || start !== end) return null;

    const rect = range.getBoundingClientRect();
    const prefix = range.cloneRange();
    prefix.selectNodeContents(start);
    prefix.setEnd(range.startContainer, range.startOffset);
    const offset = prefix.toString().length;
    const blockId = start.getAttribute("data-block-id")!;
    let reportSource: MarkdownSourceRange | undefined;
    if (workspace.report?.artifactId === blockId && start instanceof HTMLElement) {
      try {
        reportSource = resolveMarkdownRange(start, range, workspace.report.content);
      } catch {
        // Keep the menu available so submission can explain the unsupported span.
      }
    }
    return {
      blockId,
      text,
      left: Math.min(Math.max(rect.left + rect.width / 2, 110), window.innerWidth - 110),
      top: Math.max(rect.top, 96),
      start: reportSource?.start ?? offset,
      end: reportSource?.end ?? offset + selection.toString().length,
      reportSource
    };
  }

  function elementOf(node: Node): Element | null {
    return node instanceof Element ? node : node.parentElement;
  }

  async function engage(stance: Stance | null) {
    if (!target) return;
    opening = true;
    error = null;
    try {
      await session.open(
        { blockId: target.blockId, title: shorten(target.text), quote: target.text },
        stance
      );
      window.getSelection()?.removeAllRanges();
      target = null;
    } catch (caught) {
      error = caught instanceof Error ? caught.message : "Failed to open session";
    } finally {
      opening = false;
    }
  }

  function comment() {
    if (!target || workspace.report?.artifactId !== target.blockId) return;
    workspace.beginComment({
      artifactId: target.blockId,
      quote: target.reportSource?.quote ?? target.text,
      start: target.start,
      end: target.end
    });
    window.getSelection()?.removeAllRanges();
    target = null;
  }

  function reportAnchor() {
    if (!target || !workspace.report || workspace.report.artifactId !== target.blockId) {
      throw new Error("The selected report is no longer current");
    }
    if (!target.reportSource) throw new Error("The selected text is not an exact report span");
    return {
      reportArtifactId: workspace.report.artifactId,
      reportVersion: workspace.report.version,
      ...target.reportSource
    };
  }

  async function submitObjection(event: SubmitEvent) {
    event.preventDefault();
    const objection = objectionDraft.trim();
    if (!objection || opening) return;
    opening = true;
    error = null;
    try {
      await workspace.submitPublicObjection({
        ...reportAnchor(),
        objection,
        displayName: displayName.trim() || undefined
      });
      submitted = true;
      window.getSelection()?.removeAllRanges();
    } catch (caught) {
      error = caught instanceof Error ? caught.message : "Failed to submit objection";
    } finally {
      opening = false;
    }
  }

  function closeObjection() {
    objectionOpen = false;
    objectionDraft = "";
    displayName = "";
    submitted = false;
    target = null;
  }

  function handleSelectionChange() {
    if (objectionOpen) return;
    target = readSelection();
    if (target) error = null;
  }

  $effect(() => {
    const clear = () => {
      if (!objectionOpen) target = null;
    };
    // Capture-phase: the document scrolls inside a nested container, not the window.
    document.addEventListener("scroll", clear, true);
    return () => document.removeEventListener("scroll", clear, true);
  });
</script>

<svelte:document onselectionchange={handleSelectionChange} />

{#if target && (auth.actor || workspace.report?.artifactId === target.blockId)}
  <div class:objection={objectionOpen} class="menu" role="menu" style:left="{target.left}px" style:top="{target.top}px">
    {#if workspace.report?.artifactId === target.blockId}
      {#if auth.actor}
        <button type="button" disabled={opening} onpointerdown={(e) => e.preventDefault()} onclick={comment}>
          Comment
        </button>
      {:else if submitted}
        <span class="submitted" role="status">Objection sent for review.</span>
        <button type="button" onclick={closeObjection}>Close</button>
      {:else if objectionOpen}
        <form onsubmit={submitObjection}>
          <p>Object to this rendering</p>
          <blockquote>{shorten(target.text, 180)}</blockquote>
          <textarea bind:value={objectionDraft} maxlength="4000" required aria-label="Objection" placeholder="What does this get wrong?"></textarea>
          <input bind:value={displayName} maxlength="80" aria-label="Display name (optional and unverified)" placeholder="Display name (optional, unverified)" />
          <div class="form-actions">
            <button type="button" disabled={opening} onclick={closeObjection}>Cancel</button>
            <button type="submit" disabled={opening || !objectionDraft.trim()}>{opening ? "Sending…" : "Submit objection"}</button>
          </div>
        </form>
      {:else}
        <button
          type="button"
          disabled={opening}
          onpointerdown={(e) => e.preventDefault()}
          onclick={() => (objectionOpen = true)}
        >Object</button>
      {/if}
    {:else if auth.actor}
      <button type="button" disabled={opening} onpointerdown={(e) => e.preventDefault()} onclick={() => engage(null)}>
        Discuss
      </button>
      <button
        type="button"
        disabled={opening}
        onpointerdown={(e) => e.preventDefault()}
        onclick={() => engage("believe")}
      >
        Believe
      </button>
      <button
        type="button"
        disabled={opening}
        onpointerdown={(e) => e.preventDefault()}
        onclick={() => engage("dispute")}
      >
        Dispute
      </button>
    {/if}
    {#if error}
      <span class="menu-error" role="status">{error}</span>
    {/if}
  </div>
{/if}

<style>
  .menu {
    position: fixed;
    z-index: 20;
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
    padding: 3px;
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    background: var(--panel-2);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.4);
    transform: translate(-50%, calc(-100% - 10px));
  }

  .menu button {
    min-height: 0;
    padding: 0.3rem 0.65rem;
    border: 0;
    border-radius: 5px;
    background: none;
    color: var(--text);
    cursor: pointer;
    font-size: 0.8rem;
    font-weight: 550;
  }

  .menu button:hover {
    background: var(--hover);
  }

  .menu.objection {
    width: min(25rem, calc(100vw - 2rem));
    padding: 0.7rem;
    transform: translate(-50%, calc(-100% - 10px));
  }

  .menu form {
    display: flex;
    width: 100%;
    flex-direction: column;
    gap: 0.45rem;
  }

  .menu form p,
  .menu blockquote {
    margin: 0;
  }

  .menu form p {
    color: var(--text);
    font-size: 0.8rem;
    font-weight: 650;
  }

  .menu blockquote {
    color: var(--subtle);
    font-family: var(--font-doc);
    font-size: 0.72rem;
    line-height: 1.4;
  }

  .menu textarea,
  .menu input {
    box-sizing: border-box;
    width: 100%;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--panel);
    color: var(--text);
    font: inherit;
  }

  .menu textarea {
    min-height: 5rem;
    padding: 0.5rem;
    resize: vertical;
  }

  .menu input {
    padding: 0.4rem 0.5rem;
  }

  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.3rem;
  }

  .submitted {
    padding: 0.3rem 0.45rem;
    color: var(--text);
    font-size: 0.78rem;
  }

  .menu-error {
    flex-basis: 100%;
    padding: 0.2rem 0.45rem;
    color: var(--error);
    font-size: 0.72rem;
    line-height: 1.35;
  }
</style>
