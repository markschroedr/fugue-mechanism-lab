<script lang="ts">
  import { LockKeyhole } from "lucide-svelte";
  import type { BoardPost } from "../runtime";

  let { post, spaceName }: { post: BoardPost; spaceName?: string } = $props();

  function timestamp(value: string): string {
    const date = new Date(value);
    return Number.isNaN(date.valueOf())
      ? value
      : new Intl.DateTimeFormat(undefined, {
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit"
        }).format(date);
  }
</script>

<article class="society-post" data-sealed={post.sealed}>
  <div class="post-meta">
    <span class="reference">{post.reference}</span>
    <span class="author">{post.authorAddress}</span>
    <span>{post.kind}</span>
    {#if spaceName}<span>{spaceName}</span>{/if}
    <time datetime={post.createdAt}>{timestamp(post.createdAt)}</time>
  </div>
  {#if post.parentReference}<div class="reply-reference">Reply to {post.parentReference}</div>{/if}
  <h2>{post.headline}</h2>
  {#if post.recipientAddresses.length > 0}
    <div class="stakes">
      to {post.recipientAddresses.join(", ")}
      {#if post.attachedObols > 0} · {post.attachedObols} Obols each{/if}
    </div>
  {/if}
  {#if post.sealed}
    <div class="withheld"><LockKeyhole size={14} aria-hidden="true" /> Withheld · {post.attachedObols} Obols</div>
  {:else if post.body?.type === "text"}
    <p class="body-text">{post.body.text}</p>
  {:else if post.body?.type === "link"}
    <a class="body-link" href={post.body.url} target="_blank" rel="noreferrer">{post.body.url}</a>
  {:else if post.body?.type === "file"}
    <div class="body-file">{post.body.filename} · {post.body.mediaType}</div>
  {/if}
</article>

<style>
  .society-post {
    padding: 1.35rem 0 1.5rem;
    border-bottom: 1px solid var(--border);
  }

  .post-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem 0.65rem;
    color: var(--subtle);
    font-size: 0.72rem;
  }

  .author {
    color: var(--muted);
    font-family: var(--font-mono);
  }

  .reference {
    color: var(--accent);
    font-family: var(--font-mono);
  }

  .reply-reference {
    margin-top: 0.35rem;
    color: var(--subtle);
    font-family: var(--font-mono);
    font-size: 0.72rem;
  }

  h2 {
    margin: 0.45rem 0 0;
    color: var(--text);
    font-size: 1rem;
    font-weight: 650;
    letter-spacing: -0.01em;
  }

  .stakes,
  .body-file {
    margin-top: 0.35rem;
    color: var(--subtle);
    font-size: 0.76rem;
  }

  .body-text {
    max-width: 72ch;
    margin: 0.75rem 0 0;
    color: #cbc7bf;
    line-height: 1.62;
    white-space: pre-wrap;
  }

  .body-link {
    display: inline-block;
    margin-top: 0.7rem;
    color: var(--accent);
    text-decoration: underline;
    text-decoration-color: var(--border-strong);
    text-underline-offset: 3px;
    overflow-wrap: anywhere;
  }

  .withheld {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    width: fit-content;
    margin-top: 0.8rem;
    border: 1px solid var(--border);
    border-radius: 5px;
    color: var(--muted);
    font-size: 0.76rem;
    padding: 0.45rem 0.6rem;
  }
</style>
