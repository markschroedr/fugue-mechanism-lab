<script lang="ts">
  import { Tabs } from "bits-ui";
  import Check from "lucide-svelte/icons/check";
  import Plus from "lucide-svelte/icons/plus";
  import X from "lucide-svelte/icons/x";
  import EntityBlock from "./EntityBlock.svelte";
  import HighlightedText from "./HighlightedText.svelte";
  import HolderAttribution from "./HolderAttribution.svelte";
  import SelectionMenu from "./SelectionMenu.svelte";
  import SessionThread from "./SessionThread.svelte";
  import { review } from "../lib/review.svelte";
  import { session } from "../lib/session.svelte";
  import { auth } from "../lib/auth.svelte";
  import { workspace } from "../lib/workspace.svelte";
  import { renderReportMarkdown } from "../lib/markdown";
  import type { DocNode, EntityRole, ImportedSource, ReportVersionDiff } from "../lib/types";

  type EntityNode = Extract<DocNode, { kind: "entity" }>;
  type NestedSource = { source: ImportedSource; depth: number };

  let entityDraft = $state("");
  let entityRole = $state<EntityRole>("issue");
  let proposingEntity = $state(false);
  let entityError = $state<string | null>(null);
  let view = $state<"report" | "structure">("structure");
  let editOpen = $state(false);
  let editInstruction = $state("");
  let historyOpen = $state(false);
  let selectedHistoryVersion = $state<number | null>(null);
  let selectedDiff = $state<ReportVersionDiff | null>(null);
  let cadenceSeconds = $state<number | "">("");
  let savingCadence = $state(false);
  let selectedProvenanceId = $state<string | null>(null);
  let provenanceReportId = $state<string | null>(null);

  const selectedVersion = $derived(
    workspace.reportHistory.find((version) => version.version === selectedHistoryVersion) ?? null
  );
  const selectedProvenance = $derived(
    workspace.report?.provenanceAnchors?.find((anchor) => anchor.id === selectedProvenanceId) ?? null
  );
  const heldPositions = $derived(
    workspace.doc?.nodes.filter(
      (node): node is EntityNode => node.kind === "entity" && node.holders.length > 0
    ) ?? []
  );
  const nestedSources = $derived.by(() => nestSources(workspace.sources));

  function nestSources(sources: ImportedSource[]): NestedSource[] {
    const ids = new Set(sources.map((source) => source.artifactId));
    const children = new Map<string, ImportedSource[]>();
    const roots: ImportedSource[] = [];
    for (const source of sources) {
      if (!source.parentArtifactId || !ids.has(source.parentArtifactId)) {
        roots.push(source);
      } else {
        const siblings = children.get(source.parentArtifactId) ?? [];
        siblings.push(source);
        children.set(source.parentArtifactId, siblings);
      }
    }

    const nested: NestedSource[] = [];
    const visited = new Set<string>();
    function visit(source: ImportedSource, depth: number) {
      if (visited.has(source.artifactId)) return;
      visited.add(source.artifactId);
      nested.push({ source, depth });
      for (const child of children.get(source.artifactId) ?? []) visit(child, depth + 1);
    }
    for (const root of roots) visit(root, 0);
    for (const source of sources) visit(source, 0);
    return nested;
  }

  $effect(() => {
    const schedule = workspace.refreshSchedule;
    cadenceSeconds = schedule ? (schedule.enabled ? schedule.cadenceSeconds : 0) : "";
  });

  $effect(() => {
    workspace.activeSpaceId;
    historyOpen = false;
    selectedHistoryVersion = null;
    selectedDiff = null;
  });

  $effect(() => {
    const reportId = workspace.report?.artifactId ?? null;
    if (reportId) view = "report";
    if (reportId !== provenanceReportId) {
      provenanceReportId = reportId;
      selectedProvenanceId = null;
    }
  });

  /**
   * Watches the session anchor's position: while it is on screen the session
   * renders inline right below it; once it leaves, the thread docks into the
   * right flank. The margins keep top bar and composer out of the equation.
   */
  function anchorProbe(node: HTMLElement) {
    const observer = new IntersectionObserver(
      ([entry]) => {
        session.anchorInView = entry.isIntersecting;
      },
      { rootMargin: "-64px 0px -88px 0px", threshold: 0 }
    );
    observer.observe(node);
    return {
      destroy() {
        observer.disconnect();
        session.anchorInView = true;
      }
    };
  }

  function provenanceInteraction(node: HTMLElement) {
    const handle = (event: MouseEvent | KeyboardEvent) => revealProvenance(event);
    node.addEventListener("click", handle);
    node.addEventListener("keydown", handle);
    return { destroy: () => {
      node.removeEventListener("click", handle);
      node.removeEventListener("keydown", handle);
    } };
  }

  function revealProvenance(event: MouseEvent | KeyboardEvent) {
    if (event instanceof KeyboardEvent && event.key !== "Enter" && event.key !== " ") return;
    const element = event.target instanceof Element ? event.target.closest<HTMLElement>("[data-provenance-id]") : null;
    if (!element) return;
    if (event instanceof KeyboardEvent) event.preventDefault();
    const id = element.dataset.provenanceId ?? null;
    selectedProvenanceId = selectedProvenanceId === id ? null : id;
  }

  async function proposeEntity(event: SubmitEvent) {
    event.preventDefault();
    if (proposingEntity) return;
    const text = entityDraft.trim();
    if (!text) return;

    proposingEntity = true;
    entityError = null;
    try {
      await workspace.proposeEntity(entityRole, text);
      await review.load();
      entityDraft = "";
      entityRole = "issue";
    } catch (caught) {
      entityError = caught instanceof Error ? caught.message : "Failed to propose entity";
    } finally {
      proposingEntity = false;
    }
  }

  async function toggleHistory() {
    historyOpen = !historyOpen;
    if (!historyOpen) return;
    try {
      await workspace.loadReportHistory();
      try {
        await workspace.loadFeed();
      } catch {
        // Version history remains useful when the optional change feed is unavailable.
      }
      const version = workspace.reportHistory.at(-1)?.version ?? null;
      if (version) await selectHistoryVersion(version);
      else {
        selectedHistoryVersion = null;
        selectedDiff = null;
      }
    } catch {
      historyOpen = false;
    }
  }

  async function selectHistoryVersion(version: number) {
    const spaceId = workspace.activeSpaceId;
    selectedHistoryVersion = version;
    selectedDiff = null;
    const diff = await workspace.loadReportVersionDiff(version);
    if (workspace.activeSpaceId === spaceId && selectedHistoryVersion === version) selectedDiff = diff;
  }

  async function saveCadence() {
    if (savingCadence) return;
    savingCadence = true;
    try {
      if (cadenceSeconds === "") return;
      await workspace.putRefreshSchedule(cadenceSeconds || 86_400, cadenceSeconds > 0);
    } finally {
      savingCadence = false;
    }
  }

  async function synthesizeFull() {
    try {
      await workspace.synthesize("full");
      view = "report";
    } catch {
      // workspace.error is rendered above the document.
    }
  }

  async function synthesizeSelective(event: SubmitEvent) {
    event.preventDefault();
    const instruction = editInstruction.trim();
    if (!instruction) return;
    try {
      await workspace.synthesize("selective", instruction);
      editInstruction = "";
      editOpen = false;
      view = "report";
    } catch {
      // workspace.error is rendered above the document.
    }
  }
</script>

<Tabs.Root class="document" aria-label="Living synthesis document" bind:value={view}>
  <SelectionMenu />
  {#if workspace.error}
    <p class="inline-error" role="status">{workspace.error}</p>
  {/if}
  {#if workspace.doc}
    <header class="doc-head">
      <p class="eyebrow">
        {workspace.activeSpace?.title}
        · {workspace.report && view === "report" ? `report v${workspace.report.version}` : `structure v${workspace.doc.version}`}
      </p>
      <h1>{workspace.doc.title}</h1>
      <div class="doc-actions">
        {#if workspace.report}
          <Tabs.List class="view-switch" aria-label="Document view">
            <Tabs.Trigger value="report">Report</Tabs.Trigger>
            <Tabs.Trigger value="structure">Structure</Tabs.Trigger>
          </Tabs.List>
        {/if}
        {#if auth.actor}
          <button class="quiet" type="button" disabled={workspace.synthesizing} onclick={synthesizeFull}>
            {workspace.synthesizing ? "Synthesizing…" : workspace.report ? "Rewrite" : "Synthesize"}
          </button>
          {#if workspace.report}
            <button class="quiet" type="button" disabled={workspace.synthesizing} onclick={() => (editOpen = !editOpen)}>
              Edit report
            </button>
          {/if}
        {/if}
        {#if workspace.report}<button class="quiet" type="button" onclick={toggleHistory}>What changed</button>{/if}
      </div>
    </header>

    {#if auth.actor && editOpen && workspace.report}
      <form class="report-edit" onsubmit={synthesizeSelective}>
        <input bind:value={editInstruction} aria-label="Selective report edit" placeholder="Describe one targeted change" />
        <button class="quiet" type="submit" disabled={!editInstruction.trim() || workspace.synthesizing}>Apply</button>
      </form>
    {/if}

    {#if historyOpen && workspace.report}
      <section class="history" aria-label="Report version history">
        <div class="feed-head">
          <strong>Change feed</strong>
          {#if workspace.newFeedEventIds.length > 0}
            <span>{workspace.newFeedEventIds.length} since your last visit</span>
          {:else}
            <span>Nothing new since your last visit</span>
          {/if}
        </div>
        <ol class="feed">
          {#each workspace.feed as entry (entry.eventId)}
            <li class:new={workspace.newFeedEventIds.includes(entry.eventId)}>
              <span class="feed-mark" aria-hidden="true">{entry.kind === "version-written" ? "V" : entry.kind === "objection-resolved" ? "O" : "C"}</span>
              <div>
                <strong>{entry.label}</strong>
                {#if entry.objectionOutcome}<small>{entry.objectionOutcome}</small>{/if}
                {#if entry.cause?.source}
                  {#if entry.cause.source.url}
                    <a href={entry.cause.source.url} target="_blank" rel="noreferrer">{entry.cause.source.label}</a>
                  {:else}
                    <button class="source-link" type="button" onclick={() => workspace.previewSource(entry.cause!.source!.artifactId)}>{entry.cause.source.label}</button>
                  {/if}
                {/if}
                <time>{entry.createdAt}</time>
              </div>
            </li>
          {/each}
        </ol>
        {#if auth.actor}
          <div class="cadence">
            <label for="refresh-cadence">Automatic refresh</label>
            <select id="refresh-cadence" bind:value={cadenceSeconds}>
              <option value="" disabled>Choose cadence…</option>
              <option value={0}>Off</option>
              <option value={86_400}>Daily</option>
              <option value={604_800}>Weekly</option>
              <option value={2_592_000}>Monthly</option>
            </select>
            <button class="quiet" type="button" disabled={savingCadence || cadenceSeconds === ""} onclick={saveCadence}>{savingCadence ? "Saving…" : "Save"}</button>
            {#if workspace.refreshSchedule?.enabled}
              <small>Next refresh {workspace.refreshSchedule.nextDueAt}</small>
            {/if}
          </div>
        {/if}
        <nav aria-label="Report versions">
          {#each workspace.reportHistory as version (version.artifactId)}
            <button
              type="button"
              class:on={version.version === selectedHistoryVersion}
              onclick={() => void selectHistoryVersion(version.version)}
            >v{version.version}</button>
          {/each}
        </nav>
        {#if selectedVersion}
          <p class="history-meta">{selectedVersion.mode} · {selectedVersion.createdAt}</p>
          {#if selectedVersion.routingReason}
            <p class="history-meta">Routing: {selectedVersion.routingReason}</p>
          {/if}
          <div class="causes">
            <span>Caused by</span>
            {#if selectedVersion.causedBy.length === 0}
              <em>No newly accepted inputs</em>
            {:else}
              {#each selectedVersion.causedBy as cause (cause.eventId)}
                <span>
                  {cause.label}
                  {#if cause.changeClassification}
                    · <strong>{cause.changeClassification}</strong>
                  {/if}
                  {#if cause.source}
                    · {#if cause.source.url}
                      <a href={cause.source.url} target="_blank" rel="noreferrer">{cause.source.label}</a>
                    {:else}
                      <button class="source-link" type="button" onclick={() => workspace.previewSource(cause.source!.artifactId)}>{cause.source.label}</button>
                    {/if}
                  {/if}
                  {#if cause.unverifiedDisplayName}
                    · {cause.unverifiedDisplayName} <em>(unverified)</em>
                  {/if}
                  {#if cause.objectionAnchor}
                    <small>Anchored to report v{cause.objectionAnchor.reportVersion}, characters {cause.objectionAnchor.start}–{cause.objectionAnchor.end}: “{cause.objectionAnchor.quote}”</small>
                  {/if}
                  {#if cause.classificationReason}
                    <small>{cause.classificationReason}</small>
                  {/if}
                </span>
              {/each}
            {/if}
          </div>
          {#if selectedVersion.version > 1 && selectedDiff}
            <div class="diff-hunks" aria-label={`Changes from version ${selectedVersion.version - 1} to ${selectedVersion.version}`}>
              {#each selectedDiff.hunks as hunk, index (`${selectedVersion.version}-${index}`)}
                <section class="diff-hunk">
                  <pre class="diff">{#each hunk.lines as line}{line.kind === "added" ? "+" : line.kind === "removed" ? "−" : " "} <span class:added={line.kind === "added"} class:removed={line.kind === "removed"}>{line.text}</span>{"\n"}{/each}</pre>
                  <div class="hunk-causes">
                    <strong>Produced by</strong>
                    {#if hunk.causedBy.length === 0}
                      <span>No newly accepted inputs</span>
                    {:else}
                      {#each hunk.causedBy as cause (cause.eventId)}
                        <span>{cause.label}{#if cause.changeClassification} · {cause.changeClassification}{/if}</span>
                      {/each}
                    {/if}
                  </div>
                </section>
              {/each}
            </div>
          {:else if selectedVersion.version === 1}
            <p class="history-empty">First published version.</p>
          {/if}
        {/if}
      </section>
    {/if}

    {#if workspace.sourcePreview}
      <aside class="source-preview" aria-label="Source artifact">
        <button type="button" aria-label="Close source" onclick={() => (workspace.sourcePreview = null)}>
          <X size={16} strokeWidth={1.6} aria-hidden="true" />
        </button>
        <p>{workspace.sourcePreview.content}</p>
      </aside>
    {/if}

    {#if workspace.report}
      <Tabs.Content value="report" class="document-view">
      <article
        class="report-copy"
        data-block-id={workspace.report.artifactId}
        use:provenanceInteraction
      >
        {@html renderReportMarkdown(workspace.report.content, workspace.report.provenanceAnchors ?? [])}
      </article>
      {#if selectedProvenance}
        <aside class="claim-sources" aria-label="Sources backing selected report claim">
          <div>
            <span>Sources</span>
            <button type="button" aria-label="Close claim sources" onclick={() => (selectedProvenanceId = null)}>
              <X size={16} strokeWidth={1.6} aria-hidden="true" />
            </button>
          </div>
          {#each selectedProvenance.sources as source (source.artifactId)}
            {#if source.url}
              <a href={source.url} target="_blank" rel="noreferrer">{source.label}</a>
            {:else}
              <button class="source-link" type="button" onclick={() => workspace.previewSource(source.artifactId)}>{source.label}</button>
            {/if}
          {/each}
        </aside>
      {/if}
      {#if heldPositions.length > 0}
        <section class="landscape" aria-label="Who holds what">
          <h2>Positions in this report</h2>
          {#each heldPositions as position (position.id)}
            <p><strong>{position.text}</strong></p>
            <div class="holder-line">
              {#each position.holders as holder (holder.actorId + holder.stance)}
                <HolderAttribution {holder} targetEntityId={position.id} previewSource={(artifactId) => void workspace.previewSource(artifactId)} />
              {/each}
            </div>
          {/each}
        </section>
      {/if}
      </Tabs.Content>
    {/if}
      <Tabs.Content value="structure" class="document-view">
      {#if workspace.sources.length > 0}
        <section class="imported-thread" aria-label="Imported source thread">
          <h2>Imported thread</h2>
          <p class="source-count">{workspace.sources.length} source artifacts · authorship preserved</p>
          {#each nestedSources as nested (nested.source.artifactId)}
            {@const source = nested.source}
            <details class="source" style={`--thread-depth: ${nested.depth}`} open={source.sourceKind === "post"}>
              <summary>
                <span class="source-kind">{source.sourceKind}</span>
                <a href={source.sourceUrl} target="_blank" rel="noreferrer">{source.authorName}</a>
                <time datetime={source.postedAt}>{source.postedAt.slice(0, 10)}</time>
              </summary>
              <p>{source.content}</p>
            </details>
          {/each}
        </section>
      {/if}
      {#if auth.actor}<form class="entity-proposal" onsubmit={proposeEntity}>
      <select bind:value={entityRole} aria-label="Entity role" disabled={proposingEntity}>
        <option value="issue">Issue</option>
        <option value="goal">Goal</option>
        <option value="action">Action</option>
        <option value="condition">Condition</option>
      </select>
      <input
        bind:value={entityDraft}
        placeholder="Add an issue, goal, action, or condition"
        aria-label="New entity text"
        disabled={proposingEntity}
      />
      <button class="quiet" type="submit" disabled={!entityDraft.trim() || proposingEntity} aria-label="Propose entity">
        <Plus size={16} strokeWidth={1.6} aria-hidden="true" />
      </button>
      </form>{/if}
      {#if entityError}
        <p class="inline-error entity-error" role="status">{entityError}</p>
      {/if}

      {#if auth.actor && session.anchor?.blockId === "doc-root"}
      <div class="anchor-probe" use:anchorProbe></div>
      {#if session.anchorInView}
        <SessionThread variant="inline" />
      {/if}
      {/if}

      {#each workspace.doc.nodes as node (node.id)}
      {#if node.kind === "heading"}
        <h2>{node.text}</h2>
      {:else if node.kind === "prose"}
        <p class="prose" data-block-id={node.id}>
          <HighlightedText blockId={node.id} text={node.text} />
        </p>
      {:else if node.kind === "commit"}
        <p class="commit-mark">
          <span class="commit-label">
            <Check size={13} strokeWidth={1.8} aria-hidden="true" />
            {node.label} · @{node.author}
          </span>
          <span class="commit-summary">{node.summary}</span>
        </p>
      {:else if node.kind === "entity"}
        <EntityBlock
          {node}
          focused={workspace.focusedBlockId === node.id}
          onFocusBlock={(id) => workspace.focusBlock(id)}
        />
      {/if}

      {#if auth.actor && session.anchor?.blockId === node.id}
        <div class="anchor-probe" use:anchorProbe></div>
        {#if session.anchorInView}
          <SessionThread variant="inline" />
        {/if}
      {/if}
      {/each}
      </Tabs.Content>
  {:else}
    <div class="empty">
      {#if workspace.spaces.length === 0}
        <p class="eyebrow">Start here</p>
        <h1>Create a space</h1>
        <p class="prose">{auth.actor ? "Use the + beside Belief, then paste a LessWrong URL into the composer." : "Sign in below, create a space, then paste a LessWrong URL."}</p>
      {:else}
        <p class="eyebrow">{workspace.activeSpace?.title}</p>
        <h1>No synthesis yet</h1>
        <p class="prose">Paste a LessWrong URL into the composer — filing is agent work.</p>
      {/if}
    </div>
  {/if}
</Tabs.Root>

<style>
  :global(.document) {
    width: min(100%, 72ch);
    margin: 0 auto;
    padding: 3.5rem 2rem 30vh;
  }

  .doc-head {
    margin-bottom: 2.4rem;
  }

  .doc-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.45rem;
    align-items: center;
    margin-top: 1.15rem;
  }

  :global(.view-switch) {
    display: flex;
    padding: 2px;
    border: 1px solid var(--border);
    border-radius: 7px;
  }

  :global(.view-switch) :global(button) {
    min-height: 0;
    padding: 0.3rem 0.55rem;
    border: 0;
    background: transparent;
    color: var(--subtle);
    font-size: 0.74rem;
  }

  :global(.view-switch) :global(button[data-state="active"]) {
    background: var(--hover);
    color: var(--text);
  }

  .report-copy {
    color: var(--muted);
    font-family: var(--font-doc);
    font-size: 1.04rem;
    line-height: 1.75;
  }

  .report-copy :global(h1) {
    margin: 0 0 1.4rem;
    color: var(--text);
    font-size: 1.55rem;
    font-weight: 600;
    line-height: 1.3;
  }

  .report-copy :global(h2) {
    margin: 2.2rem 0 0.9rem;
    color: var(--text);
    font-size: 1.2rem;
    font-weight: 600;
    line-height: 1.35;
  }

  .report-copy :global(h3) {
    margin: 1.7rem 0 0.6rem;
    color: var(--text);
    font-size: 1.04rem;
    font-weight: 600;
    line-height: 1.4;
  }

  .report-copy :global(p) {
    margin: 0 0 1rem;
  }

  .report-copy :global(ul),
  .report-copy :global(ol) {
    margin: 0 0 1rem;
    padding-left: 1.3rem;
  }

  .report-copy :global(li) {
    margin: 0.3rem 0;
  }

  .report-copy :global(a) {
    color: var(--accent-dim);
    text-decoration: underline;
    text-decoration-color: var(--border-strong);
    text-underline-offset: 3px;
    overflow-wrap: anywhere;
  }

  .report-copy :global(a:hover) {
    color: var(--text);
  }

  .report-copy :global(.report-provenance) {
    border-bottom: 1px dotted transparent;
    cursor: help;
    transition: border-color 120ms ease, color 120ms ease;
  }

  .report-copy :global(.report-provenance:hover),
  .report-copy :global(.report-provenance:focus-visible) {
    border-color: var(--border-strong);
    color: var(--text);
    outline: none;
  }

  .claim-sources {
    position: fixed;
    right: 1rem;
    bottom: 1rem;
    z-index: 12;
    display: flex;
    width: min(24rem, calc(100vw - 2rem));
    flex-wrap: wrap;
    gap: 0.3rem 0.75rem;
    box-sizing: border-box;
    padding: 0.75rem;
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    background: var(--panel-2);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.35);
    color: var(--subtle);
    font-size: 0.72rem;
  }

  .claim-sources > div {
    display: flex;
    flex-basis: 100%;
    justify-content: space-between;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  .claim-sources button {
    min-height: 0;
    padding: 0;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
  }

  .claim-sources a { color: var(--accent-dim); }

  .report-copy :global(blockquote) {
    margin: 0 0 1rem;
    padding-left: 1rem;
    border-left: 2px solid var(--border-strong);
    color: var(--subtle);
  }

  .report-copy :global(code) {
    font-family: var(--font-mono);
    font-size: 0.88em;
  }

  .report-edit {
    display: flex;
    gap: 0.45rem;
    margin: -1rem 0 1.5rem;
    padding-bottom: 0.8rem;
    border-bottom: 1px solid var(--border);
  }

  .report-edit input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    color: var(--text);
    outline: none;
  }

  .history {
    margin: -1rem 0 2rem;
    padding: 0.8rem 0 1rem;
    border-block: 1px solid var(--border);
  }

  .feed-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 0.65rem;
    font-size: 0.74rem;
  }

  .feed-head strong {
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .feed-head span { color: var(--subtle); }

  .feed {
    display: grid;
    gap: 0.15rem;
    max-height: 13rem;
    margin: 0 0 0.8rem;
    padding: 0;
    overflow: auto;
    list-style: none;
  }

  .feed li {
    display: grid;
    grid-template-columns: 1.4rem 1fr;
    gap: 0.45rem;
    padding: 0.38rem 0;
    color: var(--muted);
    font-size: 0.74rem;
  }

  .feed li.new { color: var(--text); }

  .feed-mark {
    display: grid;
    width: 1.2rem;
    height: 1.2rem;
    place-items: center;
    border: 1px solid var(--border-strong);
    border-radius: 50%;
    color: var(--subtle);
    font-size: 0.58rem;
    font-weight: 700;
  }

  .feed li > div { display: flex; flex-wrap: wrap; gap: 0.25rem 0.45rem; align-items: baseline; }
  .feed li strong { font-weight: 600; }
  .feed li small { color: var(--subtle); text-transform: capitalize; }
  .feed time { margin-left: auto; color: var(--subtle); font-size: 0.66rem; }
  .feed a { color: var(--accent-dim); }

  .cadence {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    margin: 0.25rem 0 0.8rem;
    color: var(--subtle);
    font-size: 0.72rem;
  }

  .cadence select {
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
    font: inherit;
  }

  .cadence small { margin-left: auto; }

  .history nav {
    display: flex;
    gap: 0.25rem;
  }

  .history nav button {
    min-height: 0;
    padding: 0.22rem 0.45rem;
    border: 0;
    background: transparent;
    color: var(--subtle);
    font-size: 0.74rem;
  }

  .history nav button.on {
    color: var(--text);
    background: var(--hover);
  }

  .history-meta,
  .history-empty {
    color: var(--subtle);
    font-size: 0.72rem;
  }

  .causes {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    margin: 0.8rem 0;
    color: var(--muted);
    font-size: 0.76rem;
  }

  .causes > span:first-child {
    color: var(--subtle);
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .causes em {
    color: var(--subtle);
    font-style: normal;
  }

  .causes strong {
    color: var(--text);
    font-weight: 650;
  }

  .causes small {
    display: block;
    color: var(--subtle);
    margin-top: 0.1rem;
  }

  .causes a,
  .source-link {
    color: var(--accent-dim);
    text-underline-offset: 2px;
  }

  .source-link {
    min-height: 0;
    padding: 0;
    border: 0;
    background: transparent;
    font: inherit;
    text-decoration: underline;
  }

  .source-preview {
    position: relative;
    margin: -1rem 0 2rem;
    padding: 0.8rem 2rem 0.8rem 0;
    border-block: 1px solid var(--border);
  }

  .source-preview > button {
    position: absolute;
    top: 0.5rem;
    right: 0;
    min-height: 0;
    border: 0;
    background: transparent;
    color: var(--subtle);
  }

  .source-preview p {
    max-height: 12rem;
    margin: 0;
    overflow: auto;
    color: var(--muted);
    font-family: var(--font-doc);
    font-size: 0.82rem;
    line-height: 1.55;
    white-space: pre-wrap;
  }

  .diff-hunks { display: grid; gap: 0.7rem; }

  .diff-hunk { border: 1px solid var(--border); }

  .diff {
    max-height: 22rem;
    margin: 0;
    padding: 0.8rem;
    overflow: auto;
    border: 0;
    background: transparent;
    color: var(--subtle);
    font-family: var(--font-doc);
    font-size: 0.72rem;
    line-height: 1.5;
    white-space: pre-wrap;
  }

  .diff .added { color: var(--text); font-weight: 650; }
  .diff .removed { color: var(--subtle); text-decoration: line-through; }

  .hunk-causes {
    display: flex;
    flex-wrap: wrap;
    gap: 0.2rem 0.55rem;
    padding: 0.45rem 0.8rem;
    border-top: 1px solid var(--border);
    color: var(--subtle);
    font-size: 0.68rem;
  }

  .hunk-causes strong { color: var(--muted); text-transform: uppercase; letter-spacing: 0.06em; }

  .landscape {
    margin-top: 3rem;
    padding-top: 0.2rem;
    border-top: 1px solid var(--border);
  }

  .landscape h2 { margin-top: 1.5rem; }
  .landscape p { margin: 0.75rem 0 0; font-family: var(--font-doc); font-size: 0.88rem; line-height: 1.5; }
  .landscape .holder-line { display: grid; gap: 0.18rem; margin-top: 0.2rem; color: var(--subtle); font-family: inherit; font-size: 0.76rem; }

  .imported-thread {
    margin: -1rem 0 2.2rem;
    padding-bottom: 1rem;
    border-bottom: 1px solid var(--border);
  }

  .imported-thread h2 {
    margin-top: 0;
  }

  .source-count {
    margin: -0.4rem 0 0.8rem;
    color: var(--subtle);
    font-size: 0.75rem;
  }

  .source summary time {
    margin-left: auto;
    color: var(--subtle);
    font-size: 0.68rem;
  }

  .source {
    margin-left: calc(var(--thread-depth) * 0.9rem);
    padding: 0.45rem 0;
    border-top: 1px solid var(--border);
  }

  .source summary {
    cursor: pointer;
    color: var(--muted);
    font-size: 0.82rem;
  }

  .source summary a {
    color: inherit;
    text-decoration: none;
  }

  .source-kind {
    display: inline-block;
    width: 4.5rem;
    color: var(--subtle);
    font-size: 0.66rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .source p {
    max-height: 13rem;
    margin: 0.65rem 0 0 4.5rem;
    overflow: auto;
    color: var(--muted);
    font-family: var(--font-doc);
    font-size: 0.88rem;
    line-height: 1.55;
    white-space: pre-wrap;
  }

  .entity-proposal {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: 0.45rem;
    align-items: center;
    margin: -1.2rem 0 2rem;
    padding: 0.45rem 0;
    border-block: 1px solid var(--border);
  }

  .entity-proposal select,
  .entity-proposal input {
    min-height: 30px;
    border: 0;
    background: transparent;
    color: var(--muted);
    outline: none;
  }

  .entity-proposal select {
    max-width: 7.2rem;
    color: var(--subtle);
    font-size: 0.78rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .entity-proposal input {
    min-width: 0;
    color: var(--text);
  }

  .entity-proposal input::placeholder {
    color: var(--subtle);
  }

  .entity-proposal button {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    font-size: 1rem;
    line-height: 1;
  }

  .entity-error {
    margin-top: -1.6rem;
    margin-bottom: 1.4rem;
  }

  .eyebrow {
    margin: 0 0 0.6rem;
    color: var(--subtle);
    font-size: 0.7rem;
    font-weight: 700;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }

  h1 {
    margin: 0;
    font-family: var(--font-doc);
    font-size: clamp(1.8rem, 3.4vw, 2.4rem);
    font-weight: 600;
    line-height: 1.15;
    text-wrap: balance;
  }

  h2 {
    margin: 2.6rem 0 0.8rem;
    font-family: var(--font-doc);
    font-size: 1.25rem;
    font-weight: 600;
  }

  .prose {
    margin: 0.7rem 0;
    color: var(--muted);
    font-family: var(--font-doc);
    font-size: 1.02rem;
    line-height: 1.7;
  }

  .commit-mark {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
    margin: 1.4rem 0;
  }

  .commit-label {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    flex-shrink: 0;
    color: var(--accent-dim);
    font-family: var(--font-mono);
    font-size: 0.7rem;
    letter-spacing: 0.03em;
  }

  .commit-summary {
    color: var(--subtle);
    font-size: 0.76rem;
  }

  .anchor-probe {
    height: 1px;
  }

  .empty {
    padding-top: 3rem;
  }
</style>
