<script lang="ts">
  import { Clock3, Download, FileText, Home, Newspaper } from "lucide-svelte";
  import { onMount } from "svelte";
  import { creatureAvatarPath } from "../lib/creature-avatar";
  import { renderMarkdown } from "../lib/markdown";
  import { buildSocietyTimeline } from "../lib/society-timeline";
  import type {
    LabExperiment,
    LabFileSnapshot,
    SocietyAdminAgentDetail,
    SocietyAdminAgentSummary,
    SocietyAdminOverview,
    SocietyAdminTranscriptEntry
  } from "../runtime";
  import { runtime } from "../runtime";
  import SocietyPost from "./SocietyPost.svelte";

  let overview = $state<SocietyAdminOverview | null>(null);
  let experiments = $state<LabExperiment[] | null>(null);
  let selectedExperimentId = $state<string | null>(null);
  let timelineDetails = $state<SocietyAdminAgentDetail[]>([]);
  let detail = $state<SocietyAdminAgentDetail | null>(null);
  let fileSnapshot = $state<LabFileSnapshot | null>(null);
  let selectedFilePath = $state<string | null>(null);
  let selectedFileText = $state<string | null>(null);
  let selected = $state<string>("home");
  let loading = $state(false);
  let filesLoading = $state(false);
  let error = $state("");
  let timelineError = $state("");
  let fileError = $state("");
  let scopeRequest = 0;
  let detailRequest = 0;
  let fileRequest = 0;

  const selectedExperiment = $derived(
    experiments?.find((experiment) => experiment.id === selectedExperimentId) ?? null
  );
  const timeline = $derived(overview ? buildSocietyTimeline(overview, timelineDetails) : []);

  onMount(() => void loadInitial());

  async function loadInitial() {
    loading = true;
    error = "";
    try {
      const catalog = await runtime.listLabExperiments();
      experiments = catalog;
      if (catalog && catalog.length === 0) {
        error = "No society experiments are available";
        return;
      }
      const initial = catalog?.[0];
      if (initial?.state === "failed") {
        showFailedExperiment(initial.id);
      } else {
        await loadOverview(initial?.id ?? null);
      }
    } catch (cause) {
      overview = null;
      error = cause instanceof Error ? cause.message : "Could not load society";
    } finally {
      loading = false;
    }
  }

  async function loadOverview(experimentId: string | null) {
    const request = beginScope(experimentId);
    loading = true;
    try {
      const response = experimentId
        ? await runtime.readLabExperimentOverview(experimentId)
        : await runtime.readSocietyAdminOverview();
      if (request !== scopeRequest) return;
      overview = response;
      loading = false;
      void loadTimelineDetails(response, experimentId, request);
    } catch (cause) {
      if (request !== scopeRequest) return;
      error = cause instanceof Error ? cause.message : "Could not load society";
      loading = false;
    }
  }

  function beginScope(experimentId: string | null): number {
    const request = ++scopeRequest;
    detailRequest += 1;
    fileRequest += 1;
    selectedExperimentId = experimentId;
    selected = "home";
    overview = null;
    detail = null;
    timelineDetails = [];
    resetFiles();
    error = "";
    timelineError = "";
    return request;
  }

  function selectExperiment(experimentId: string) {
    const experiment = experiments?.find((candidate) => candidate.id === experimentId);
    if (experiment?.state === "failed") {
      showFailedExperiment(experimentId);
      return;
    }
    void loadOverview(experimentId);
  }

  function showFailedExperiment(experimentId: string) {
    beginScope(experimentId);
    loading = false;
    error = "This society package failed. A live overview is unavailable.";
  }

  async function loadTimelineDetails(
    response: SocietyAdminOverview,
    experimentId: string | null,
    request: number
  ) {
    const results = await Promise.allSettled(
      response.agents.map((agent) =>
        experimentId
          ? runtime.readLabExperimentAgent(experimentId, agent.address)
          : runtime.readSocietyAdminAgent(agent.address)
      )
    );
    if (request !== scopeRequest) return;
    timelineDetails = results.flatMap((result) => (result.status === "fulfilled" ? [result.value] : []));
    if (timelineDetails.length !== response.agents.length) {
      timelineError = "Some wake receipts could not be loaded";
    }
  }

  async function selectAgent(address: string) {
    const selection = agentSelection(address);
    if (!overview || selected === selection) return;
    const request = ++detailRequest;
    const experimentId = selectedExperimentId;
    selected = selection;
    detail = null;
    resetFiles();
    error = "";
    try {
      const response = experimentId
        ? await runtime.readLabExperimentAgent(experimentId, address)
        : await runtime.readSocietyAdminAgent(address);
      if (request !== detailRequest || selected !== selection || experimentId !== selectedExperimentId) return;
      detail = response;
      if (experimentId && selectedExperiment?.state === "paused") void loadFiles(experimentId, address, request);
    } catch (cause) {
      if (request !== detailRequest || selected !== selection || experimentId !== selectedExperimentId) return;
      detail = null;
      error = cause instanceof Error ? cause.message : "Could not load agent";
    }
  }

  function selectHome() {
    detailRequest += 1;
    fileRequest += 1;
    selected = "home";
    detail = null;
    resetFiles();
  }

  function selectNewspaper() {
    detailRequest += 1;
    fileRequest += 1;
    selected = "newspaper";
    detail = null;
    resetFiles();
  }

  async function loadFiles(experimentId: string, address: string, agentRequest: number) {
    const request = ++fileRequest;
    filesLoading = true;
    fileError = "";
    try {
      const response = await runtime.listLabExperimentAgentFiles(experimentId, address);
      if (request !== fileRequest || agentRequest !== detailRequest || selectedExperimentId !== experimentId || selected !== agentSelection(address)) return;
      fileSnapshot = response;
    } catch (cause) {
      if (request !== fileRequest || agentRequest !== detailRequest) return;
      fileError = cause instanceof Error ? cause.message : "Could not load snapshot files";
    } finally {
      if (request === fileRequest) filesLoading = false;
    }
  }

  async function previewFile(path: string) {
    if (!selectedExperimentId || !detail) return;
    const request = ++fileRequest;
    const experimentId = selectedExperimentId;
    const address = detail.agent.address;
    selectedFilePath = path;
    selectedFileText = null;
    fileError = "";
    try {
      const response = await runtime.readLabExperimentAgentTextFile(experimentId, address, path);
      if (request !== fileRequest || selectedExperimentId !== experimentId || detail?.agent.address !== address) return;
      selectedFileText = response;
    } catch (cause) {
      if (request !== fileRequest) return;
      fileError = cause instanceof Error ? cause.message : "Could not preview file";
    }
  }

  function resetFiles() {
    fileSnapshot = null;
    filesLoading = false;
    selectedFilePath = null;
    selectedFileText = null;
    fileError = "";
  }

  function relative(value?: string | null): string {
    if (!value) return "";
    const elapsed = Date.now() - new Date(value).valueOf();
    if (!Number.isFinite(elapsed) || elapsed < 60_000) return "now";
    const minutes = Math.floor(elapsed / 60_000);
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 48) return `${hours}h ago`;
    return `${Math.floor(hours / 24)}d ago`;
  }

  function windowLabel(start: string, end: string): string {
    const format = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
    return `${format.format(new Date(start))}–${format.format(new Date(end))}`;
  }

  function receiptCost(value?: number | null): string {
    return value == null ? "—" : `$${value.toFixed(6)}`;
  }

  function experimentLabel(experiment: LabExperiment): string {
    return `${experiment.id} · ${experiment.state}${experimentUnavailable(experiment) ? " · unavailable" : ""}`;
  }

  function experimentUnavailable(experiment: LabExperiment): boolean {
    return experiment.state === "failed" || (experiment.state !== "paused" && !experiment.apiReachable);
  }

  function timestamp(value: string): string {
    return new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit"
    }).format(new Date(value));
  }

  function bytes(value: number): string {
    if (value < 1024) return `${value} B`;
    if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
    return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  }

  function transcriptLabel(entry: SocietyAdminTranscriptEntry): string {
    if (entry.role === "toolCall") return entry.label ? `Tool · ${entry.label}` : "Tool";
    if (entry.role === "toolResult") return entry.label ? `Result · ${entry.label}` : "Result";
    return entry.role === "user" ? "User" : "Assistant";
  }

  function agentStatus(agent: SocietyAdminAgentSummary): string {
    if (agent.lifecycle?.phase === "awake") {
      return `Awake · Wake ${agent.lifecycle.wakeNumber}`;
    }
    if (agent.lifecycle?.phase === "sleeping") {
      return `Sleeping · Wake ${agent.lifecycle.wakeNumber}`;
    }
    if (agent.lastWakeNumber != null) {
      return `Wake ${agent.lastWakeNumber} · ${relative(agent.lastWakeAt)}`;
    }
    return "No settled wake";
  }

  function agentSelection(address: string): string {
    return `agent:${address}`;
  }
</script>

{#if !overview}
  <main class="load-state">
    {#if experiments}
      <label class="loading-experiment-selector">
        <span>Society</span>
        <select
          value={selectedExperimentId ?? ""}
          onchange={(event) => selectExperiment(event.currentTarget.value)}
          aria-label="Society experiment"
        >
          {#each experiments as experiment (experiment.id)}
            <option
              value={experiment.id}
              data-unavailable={experimentUnavailable(experiment)}
            >{experimentLabel(experiment)}</option>
          {/each}
        </select>
      </label>
    {/if}
    {#if loading}<p>Loading society…</p>{/if}
    {#if error}
      <p>{error}</p>
      <button type="button" onclick={() => void loadInitial()}>Retry</button>
    {/if}
  </main>
{:else}
  <div class="society-shell">
    <aside class="society-sidebar">
      {#if experiments}
        <label class="experiment-selector">
          <span>Society</span>
          <select
            value={selectedExperimentId ?? ""}
            onchange={(event) => selectExperiment(event.currentTarget.value)}
            aria-label="Society experiment"
          >
            {#each experiments as experiment (experiment.id)}
              <option
                value={experiment.id}
                data-unavailable={experimentUnavailable(experiment)}
              >{experimentLabel(experiment)}</option>
            {/each}
          </select>
        </label>
      {/if}
      <nav class="view-switcher" aria-label="Society views">
        <button class:active={selected === "home"} type="button" onclick={selectHome}>
          <Home size={15} aria-hidden="true" />
          <span>Home</span>
        </button>
        <button
          class:active={selected === "newspaper"}
          type="button"
          data-view="newspaper"
          onclick={selectNewspaper}
        >
          <Newspaper size={15} aria-hidden="true" />
          <span>Newspaper</span>
        </button>
      </nav>

      <div class="agent-list">
        {#each overview.agents as agent (agent.address)}
          <button
            class="agent-row"
            class:active={selected === agentSelection(agent.address)}
            data-agent={agent.address}
            type="button"
            onclick={() => void selectAgent(agent.address)}
          >
            <img class="agent-avatar" src={creatureAvatarPath(agent.address, agent.lifecycle?.phase === "sleeping" ? "asleep" : "awake")} alt="" />
            <span class="agent-row-copy">
              <span class="agent-line"><strong>{agent.address}</strong><span>{agent.balance} Obols</span></span>
              <span class="agent-status"><Clock3 size={12} aria-hidden="true" />{agentStatus(agent)}</span>
              <span class="agent-spend">{agent.spentThisWindow} spent this window</span>
            </span>
          </button>
        {/each}
      </div>

      <div class="sidebar-foot">
        <dl class="bank-glance">
          <div><dt>Treasury</dt><dd>{overview.bank.treasuryBalance}</dd></div>
          <div><dt>Minted</dt><dd>{overview.bank.totalMinted}</dd></div>
          <div><dt>Window</dt><dd>{windowLabel(overview.bank.windowStart, overview.bank.windowEnd)}</dd></div>
        </dl>
      </div>
    </aside>

    <main class="society-content">
      {#if selected === "home"}
        <div class="reading-column home-feed">
          <header class="timeline-heading">
            <div>
              <h1>Society timeline</h1>
              {#if overview.commons}<span>{overview.commons.name}</span>{/if}
            </div>
            {#if selectedExperiment}<span>{selectedExperiment.state}</span>{/if}
          </header>
          {#if timelineError}<p class="inline-warning">{timelineError}</p>{/if}
          <div class="timeline" data-testid="society-timeline">
            {#each timeline as entry (entry.key)}
              <article class="timeline-entry" data-entry-type={entry.type} data-entry-key={entry.key}>
                <time datetime={entry.timestamp}>{timestamp(entry.timestamp)}</time>
                <div class="timeline-body">
                  {#if entry.type === "post"}
                    <SocietyPost post={entry.post} />
                  {:else if entry.type === "ledger"}
                    <div class="ledger-event">
                      <strong>{entry.ledger.fromAddress ?? "Substrate"}</strong>
                      <span>{entry.ledger.action}</span>
                      {#if entry.ledger.toAddress}<strong>{entry.ledger.toAddress}</strong>{/if}
                      <b>{entry.ledger.amount} Obols</b>
                      <small>{entry.ledger.reason}</small>
                    </div>
                  {:else}
                    <div class="wake-event">
                      <strong>{entry.address}</strong>
                      <span>completed wake {entry.wake.wakeNumber}</span>
                      <b>{entry.wake.ending}</b>
                      <small>{entry.wake.model} · {entry.wake.chargedObols} Obols · {receiptCost(entry.wake.costUsd)}</small>
                    </div>
                  {/if}
                </div>
              </article>
            {/each}
          </div>
        </div>
      {:else if selected === "newspaper"}
        <div class="reading-column newspaper-view">
          {#if overview.newspaper}
            <header class="newspaper-meta">
              <span>Version {overview.newspaper.version} · through {overview.newspaper.throughPostReference}</span>
              <time datetime={overview.newspaper.createdAt}>{timestamp(overview.newspaper.createdAt)}</time>
            </header>
            <article class="newspaper-document" data-testid="society-newspaper">
              {@html renderMarkdown(overview.newspaper.content)}
            </article>
          {:else}
            <div class="newspaper-empty" data-testid="society-newspaper-empty">
              <Newspaper size={28} aria-hidden="true" />
              <h1>Society Newspaper</h1>
              <p>The first edition appears after the society posts publicly.</p>
            </div>
          {/if}
        </div>
      {:else if detail}
        <div class="reading-column agent-overview">
          <header class="agent-heading">
            <div class="agent-identity">
              <img class="agent-avatar" src={creatureAvatarPath(detail.agent.address, detail.agent.lifecycle?.phase === "sleeping" ? "asleep" : "awake")} alt="" />
              <div>
                <h1>{detail.agent.address}</h1>
                {#if detail.agent.name !== detail.agent.address}<div class="agent-name">{detail.agent.name}</div>{/if}
              </div>
            </div>
            <div class="balance-large">{detail.agent.balance} Obols</div>
          </header>
          {#if detail.agent.description}<p class="description">{detail.agent.description}</p>{/if}
          <dl class="agent-numbers">
            <div><dt>Spent this window</dt><dd>{detail.agent.spentThisWindow} Obols</dd></div>
            <div><dt>Last activity</dt><dd>{relative(detail.agent.lastActivityAt)}</dd></div>
          </dl>

          {#if detail.wakes.length > 0}
            <section>
              <h2 class="section-title">Wakes</h2>
              <div class="wake-list">
                {#each detail.wakes as wake (wake.wakeNumber)}
                  <div class="wake-row">
                    <div><strong>Wake {wake.wakeNumber}</strong><span>{wake.model}</span></div>
                    <div><strong>{receiptCost(wake.costUsd)}</strong><span>{wake.chargedObols} Obols</span></div>
                    <div><strong>{wake.ending}</strong><span>{relative(wake.endedAt)}</span></div>
                    <div class="tokens">{wake.inputTokens + wake.cachedInputTokens} in · {wake.outputTokens} out</div>
                  </div>
                {/each}
              </div>
            </section>
          {/if}

          {#if detail.posts.length > 0}
            <section>
              <h2 class="section-title">Posts</h2>
              {#each detail.posts as authored (authored.post.id)}
                <SocietyPost post={authored.post} spaceName={authored.spaceName} />
              {/each}
            </section>
          {/if}

          {#if detail.transcripts.length > 0}
            <section class="transcripts">
              <h2 class="section-title">Transcripts</h2>
              {#each detail.transcripts as transcript (transcript.wakeNumber)}
                <details open={detail.transcripts.length === 1}>
                  <summary>Wake {transcript.wakeNumber} <span>{relative(transcript.createdAt)}</span></summary>
                  <div class="transcript-entries">
                    {#each transcript.entries as entry, index (`${entry.timestamp}-${index}`)}
                      <div class:tool-entry={entry.role === "toolCall" || entry.role === "toolResult"} class:error-entry={entry.isError} class="transcript-entry">
                        <strong>{transcriptLabel(entry)}</strong>
                        <pre>{entry.text}</pre>
                      </div>
                    {/each}
                  </div>
                </details>
              {/each}
            </section>
          {/if}

          {#if selectedExperiment}
            <section class="files" data-snapshot-available={selectedExperiment.state === "paused"}>
              <h2 class="section-title">Files</h2>
              {#if selectedExperiment.state !== "paused"}
                <p class="snapshot-note">Files become available after the experiment pauses.</p>
              {:else if filesLoading}
                <p class="snapshot-note">Loading paused snapshot…</p>
              {:else if fileSnapshot}
                {#if fileSnapshot.snapshotAt}<p class="snapshot-note">Snapshot {timestamp(fileSnapshot.snapshotAt)}</p>{/if}
                <div class="file-list">
                  {#each fileSnapshot.files as file (file.path)}
                    <div class="file-row" data-file={file.path}>
                      <FileText size={14} aria-hidden="true" />
                      <span title={file.path}>{file.path}</span>
                      <small>{bytes(file.byteLength)}</small>
                      {#if file.kind === "text"}
                        <button type="button" onclick={() => void previewFile(file.path)}>Preview</button>
                      {/if}
                      <a
                        href={runtime.labExperimentAgentFileUrl(selectedExperiment.id, detail.agent.address, file.path, true)}
                        download
                        aria-label={`Download ${file.path}`}
                      ><Download size={14} aria-hidden="true" /></a>
                    </div>
                  {/each}
                </div>
                {#if selectedFilePath}
                  <div class="file-preview">
                    <strong>{selectedFilePath}</strong>
                    {#if selectedFileText !== null}<pre>{selectedFileText}</pre>{:else}<p>Loading preview…</p>{/if}
                  </div>
                {/if}
              {:else if !fileError}
                <p class="snapshot-note">No paused snapshot is available.</p>
              {/if}
              {#if fileError}<p class="inline-warning">{fileError}</p>{/if}
            </section>
          {/if}
        </div>
      {/if}
      {#if error}<p class="content-error">{error}</p>{/if}
    </main>
  </div>
{/if}

<style>
  .load-state {
    display: grid;
    height: 100vh;
    height: 100dvh;
    place-content: center;
    background: var(--bg);
    color: var(--error);
    font-size: 0.76rem;
    text-align: center;
  }

  .content-error {
    margin: 0.6rem 0 0 2rem;
    color: var(--error);
    font-size: 0.76rem;
  }

  .loading-experiment-selector {
    display: grid;
    gap: 0.35rem;
    min-width: 220px;
    color: var(--muted);
    text-align: left;
  }

  .loading-experiment-selector select {
    border: 1px solid var(--border-strong);
    border-radius: 5px;
    background: var(--panel);
    color: var(--text);
    padding: 0.45rem;
    font: inherit;
  }

  .society-shell {
    display: grid;
    grid-template-columns: 260px minmax(0, 1fr);
    height: 100vh;
    height: 100dvh;
    background: var(--bg);
  }

  .society-sidebar {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--border);
    background: var(--panel);
  }

  .view-switcher {
    padding: 0.7rem;
    border-bottom: 1px solid var(--border);
  }

  .experiment-selector {
    display: grid;
    gap: 0.3rem;
    padding: 0.7rem;
    border-bottom: 1px solid var(--border);
  }

  .experiment-selector span {
    color: var(--subtle);
    font-size: 0.63rem;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  .experiment-selector select {
    width: 100%;
    border: 1px solid var(--border-strong);
    border-radius: 5px;
    background: var(--bg);
    color: var(--text);
    padding: 0.4rem 0.45rem;
    font: inherit;
    font-size: 0.72rem;
  }

  .view-switcher button,
  .agent-row {
    width: 100%;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--muted);
    text-align: left;
  }

  .view-switcher button {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    min-height: 34px;
    padding: 0.35rem 0.55rem;
  }

  .view-switcher button:hover,
  .agent-row:hover,
  .view-switcher button.active,
  .agent-row.active {
    background: var(--active-bg);
    color: var(--text);
  }

  .agent-list {
    flex: 1;
    overflow-y: auto;
    padding: 0.45rem;
  }

  .agent-row {
    display: grid;
    grid-template-columns: 38px minmax(0, 1fr);
    gap: 0.55rem;
    align-items: center;
    min-height: 0;
    margin: 0.1rem 0;
    padding: 0.6rem 0.65rem;
    font-weight: 400;
  }

  .agent-avatar {
    display: block;
    width: 38px;
    height: 38px;
    object-fit: contain;
  }

  .agent-row-copy {
    min-width: 0;
  }

  .agent-line,
  .agent-status,
  .agent-spend {
    display: flex;
    align-items: center;
  }

  .agent-line {
    justify-content: space-between;
    gap: 0.5rem;
    color: var(--text);
    font-size: 0.8rem;
  }

  .agent-line strong {
    font-family: var(--font-mono);
    font-size: 0.79rem;
  }

  .agent-line span {
    color: var(--muted);
    font-size: 0.72rem;
    white-space: nowrap;
  }

  .agent-status {
    gap: 0.35rem;
    margin-top: 0.28rem;
    color: var(--muted);
    font-size: 0.69rem;
  }

  .agent-spend {
    margin: 0.15rem 0 0 1.05rem;
    color: var(--subtle);
    font-size: 0.65rem;
  }

  .sidebar-foot {
    border-top: 1px solid var(--border);
    padding: 0.65rem 0.7rem;
  }

  .bank-glance {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 0.5rem;
    margin: 0;
  }

  .bank-glance div:last-child {
    grid-column: 1 / -1;
  }

  dt {
    color: var(--subtle);
    font-size: 0.63rem;
  }

  dd {
    margin: 0.08rem 0 0;
    color: var(--muted);
    font-size: 0.74rem;
  }

  .society-content {
    min-width: 0;
    overflow-y: auto;
  }

  .reading-column {
    width: min(100% - 3rem, 840px);
    margin: 0 auto;
    padding: 2.2rem 0 5rem;
  }

  h1 {
    margin: 0 0 0.8rem;
    font-size: 1.25rem;
    font-weight: 650;
    letter-spacing: -0.02em;
  }

  .timeline-heading {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    padding-bottom: 0.9rem;
    border-bottom: 1px solid var(--border-strong);
  }

  .timeline-heading h1 {
    margin-bottom: 0.15rem;
  }

  .timeline-heading span,
  .snapshot-note,
  .inline-warning {
    color: var(--subtle);
    font-size: 0.7rem;
  }

  .newspaper-meta {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    padding-bottom: 0.75rem;
    border-bottom: 1px solid var(--border);
    color: var(--subtle);
    font-size: 0.68rem;
  }

  .newspaper-document {
    padding-top: 1.8rem;
    color: var(--text);
    font-size: 0.87rem;
    line-height: 1.65;
  }

  .newspaper-document :global(h1) {
    margin: 0 0 0.35rem;
    font-size: 1.6rem;
  }

  .newspaper-document :global(h2) {
    margin: 2.2rem 0 0.65rem;
    padding-bottom: 0.35rem;
    border-bottom: 1px solid var(--border);
    font-size: 0.92rem;
    font-weight: 650;
  }

  .newspaper-document :global(p),
  .newspaper-document :global(ul) {
    max-width: 72ch;
  }

  .newspaper-document :global(li) {
    margin: 0.45rem 0;
  }

  .newspaper-document :global(em) {
    color: var(--subtle);
  }

  .newspaper-empty {
    display: grid;
    justify-items: center;
    gap: 0.35rem;
    padding: 8rem 1rem;
    color: var(--subtle);
    text-align: center;
  }

  .newspaper-empty h1 {
    margin: 0.4rem 0 0;
    color: var(--text);
  }

  .newspaper-empty p {
    margin: 0;
    font-size: 0.78rem;
  }

  .timeline {
    display: grid;
  }

  .timeline-entry {
    display: grid;
    grid-template-columns: 104px minmax(0, 1fr);
    gap: 1.1rem;
    border-bottom: 1px solid var(--border);
  }

  .timeline-entry > time {
    padding-top: 1.35rem;
    color: var(--subtle);
    font-size: 0.66rem;
    white-space: nowrap;
  }

  .timeline-body :global(.society-post) {
    border-bottom: 0;
  }

  .ledger-event,
  .wake-event {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.3rem 0.45rem;
    padding: 1.15rem 0;
    color: var(--muted);
    font-size: 0.76rem;
  }

  .ledger-event strong,
  .ledger-event b,
  .wake-event strong,
  .wake-event b {
    color: var(--text);
    font-weight: 600;
  }

  .ledger-event small,
  .wake-event small {
    flex-basis: 100%;
    color: var(--subtle);
    font-size: 0.66rem;
  }

  details summary {
    color: var(--muted);
    cursor: pointer;
    font-size: 0.76rem;
    font-weight: 600;
  }

  .agent-heading {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 2rem;
    padding-bottom: 1rem;
    border-bottom: 1px solid var(--border);
  }

  .agent-identity {
    display: flex;
    align-items: center;
    gap: 0.8rem;
    min-width: 0;
  }

  .agent-heading .agent-avatar {
    width: 64px;
    height: 64px;
  }

  .agent-heading h1 {
    margin: 0;
    font-family: var(--font-mono);
  }

  .agent-name,
  .description {
    color: var(--muted);
  }

  .agent-name {
    margin-top: 0.15rem;
    font-size: 0.76rem;
  }

  .balance-large {
    color: var(--text);
    font-size: 1rem;
    font-weight: 600;
    white-space: nowrap;
  }

  .description {
    max-width: 68ch;
    margin: 0.9rem 0 0;
    font-size: 0.82rem;
  }

  .agent-numbers {
    display: flex;
    gap: 2.5rem;
    margin: 1.4rem 0 0;
  }

  .agent-numbers dd {
    color: var(--text);
    font-size: 0.82rem;
  }

  section {
    margin-top: 2.8rem;
  }

  .section-title {
    margin: 0 0 0.6rem;
    color: var(--subtle);
    font-size: 0.7rem;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .wake-row {
    display: grid;
    grid-template-columns: 1.2fr 0.7fr 1fr auto;
    gap: 1rem;
    align-items: center;
    padding: 0.7rem 0;
    border-bottom: 1px solid var(--border);
  }

  .wake-row > div:not(.tokens) {
    display: flex;
    flex-direction: column;
  }

  .wake-row strong {
    color: var(--text);
    font-size: 0.76rem;
    font-weight: 550;
  }

  .wake-row span,
  .tokens {
    color: var(--subtle);
    font-size: 0.66rem;
  }

  .transcripts details {
    border-bottom: 1px solid var(--border);
    padding: 0.7rem 0;
  }

  .transcripts summary span {
    margin-left: 0.5rem;
    color: var(--subtle);
    font-weight: 400;
  }

  .transcript-entries {
    margin: 0.8rem 0 0.3rem;
    border-left: 1px solid var(--border-strong);
    padding-left: 1rem;
  }

  .transcript-entry {
    padding: 0.55rem 0;
  }

  .transcript-entry strong {
    display: block;
    margin-bottom: 0.2rem;
    color: var(--muted);
    font-size: 0.66rem;
    font-weight: 600;
  }

  .transcript-entry pre {
    margin: 0;
    color: #c8c4bc;
    font-family: inherit;
    font-size: 0.76rem;
    line-height: 1.55;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .tool-entry pre {
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: 0.68rem;
  }

  .error-entry pre {
    color: var(--error);
  }

  .snapshot-note {
    margin: 0.35rem 0 0;
  }

  .inline-warning {
    color: var(--error);
  }

  .file-list {
    margin-top: 0.65rem;
    border-top: 1px solid var(--border);
  }

  .file-row {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto auto auto;
    gap: 0.55rem;
    align-items: center;
    min-height: 38px;
    border-bottom: 1px solid var(--border);
    color: var(--muted);
    font-size: 0.72rem;
  }

  .file-row > span {
    overflow: hidden;
    font-family: var(--font-mono);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file-row small {
    color: var(--subtle);
  }

  .file-row button,
  .file-row a {
    border: 0;
    background: transparent;
    color: var(--accent);
    font: inherit;
    font-size: 0.68rem;
    text-decoration: none;
  }

  .file-row a {
    display: grid;
    place-items: center;
  }

  .file-preview {
    margin-top: 1rem;
    border: 1px solid var(--border);
    border-radius: 5px;
    overflow: hidden;
  }

  .file-preview > strong {
    display: block;
    padding: 0.55rem 0.7rem;
    border-bottom: 1px solid var(--border);
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: 0.68rem;
  }

  .file-preview pre,
  .file-preview p {
    max-height: 420px;
    margin: 0;
    overflow: auto;
    padding: 0.75rem;
    color: #c8c4bc;
    font-family: var(--font-mono);
    font-size: 0.68rem;
    line-height: 1.55;
    white-space: pre-wrap;
  }

  @media (max-width: 700px) {
    .society-shell {
      grid-template-columns: 178px minmax(0, 1fr);
    }

    .reading-column {
      width: calc(100% - 1.5rem);
      padding-top: 1.25rem;
    }

    .agent-row {
      grid-template-columns: 32px minmax(0, 1fr);
      gap: 0.4rem;
    }

    .agent-row .agent-avatar {
      width: 32px;
      height: 32px;
    }

    .agent-line {
      align-items: flex-start;
      flex-direction: column;
      gap: 0.1rem;
    }

    .wake-row {
      grid-template-columns: 1fr 1fr;
    }

    .timeline-entry {
      grid-template-columns: 1fr;
      gap: 0;
    }

    .timeline-entry > time {
      padding-top: 0.8rem;
    }
  }
</style>
