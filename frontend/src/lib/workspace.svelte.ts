import { runtime } from "../runtime";
import type { InputAnchor, InputKind, PublicObjectionInput } from "../runtime/types";
import type { ArtifactReadModel, DocNode, EntityRole, FeedEntry, ImportedSource, Projection, RefreshSchedule, ReportVersion, ReportVersionDiff, ReviewItem, SpaceRef, SynthesisDoc, SynthesisReport } from "./types";

/**
 * Workspace state: the space list grouped by projection, the active
 * synthesis document, and the cross-projection echo used by the rail.
 */
class WorkspaceState {
  spaces = $state<SpaceRef[]>([]);
  activeSpaceId = $state<string>("");
  doc = $state<SynthesisDoc | null>(null);
  report = $state<SynthesisReport | null>(null);
  reportHistory = $state<ReportVersion[]>([]);
  feed = $state<FeedEntry[]>([]);
  newFeedEventIds = $state<string[]>([]);
  refreshSchedule = $state<RefreshSchedule | null>(null);
  sourcePreview = $state<ArtifactReadModel | null>(null);
  sources = $state<ImportedSource[]>([]);
  loading = $state(false);
  synthesizing = $state(false);
  error = $state<string | null>(null);

  /** Space currently hovered in the rail; overrides the active-space echo. */
  hoveredSpaceId = $state<string | null>(null);

  /** The document block the composer will anchor a session to. */
  focusedBlockId = $state<string | null>(null);

  /** A selected report span waiting for one comment through the primary input. */
  inputAnchor = $state<InputAnchor | null>(null);

  activeSpace = $derived(this.spaces.find((space) => space.id === this.activeSpaceId) ?? null);

  /** Neighbor ids echoed in the rail: hover wins, otherwise the active space. */
  echo = $derived.by(() => {
    const sourceId = this.hoveredSpaceId ?? this.activeSpaceId;
    const source = this.spaces.find((space) => space.id === sourceId);
    return {
      sourceId,
      fromHover: this.hoveredSpaceId !== null,
      neighborIds: new Set(source?.neighbors ?? [])
    };
  });

  focusedBlock = $derived.by(() => {
    if (!this.doc || !this.focusedBlockId) return null;
    const node = this.doc.nodes.find(
      (candidate) => candidate.id === this.focusedBlockId && candidate.kind === "entity"
    );
    return node ?? null;
  });

  async load() {
    if (this.loading) return;
    this.loading = true;
    this.error = null;
    try {
      this.spaces = await runtime.listSpaces();
      if (!this.spaces.some((space) => space.id === this.activeSpaceId)) {
        this.activeSpaceId = this.spaces[0]?.id ?? "";
      }
      if (this.activeSpaceId) {
        [this.doc, this.report, this.sources, this.refreshSchedule] = await Promise.all([
          runtime.readDoc(this.activeSpaceId),
          runtime.readReport(this.activeSpaceId),
          runtime.readSources(this.activeSpaceId),
          runtime.readRefreshSchedule(this.activeSpaceId).catch(() => null)
        ]);
      } else {
        this.doc = null;
        this.report = null;
        this.reportHistory = [];
        this.feed = [];
        this.newFeedEventIds = [];
        this.refreshSchedule = null;
        this.sources = [];
      }
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to load workspace";
    } finally {
      this.loading = false;
    }
  }

  async selectSpace(id: string) {
    this.activeSpaceId = id;
    this.focusedBlockId = null;
    this.reportHistory = [];
    this.feed = [];
    this.newFeedEventIds = [];
    this.refreshSchedule = null;
    this.sourcePreview = null;
    this.error = null;
    try {
      [this.doc, this.report, this.sources, this.refreshSchedule] = await Promise.all([
        runtime.readDoc(id),
        runtime.readReport(id),
        runtime.readSources(id),
        runtime.readRefreshSchedule(id).catch(() => null)
      ]);
    } catch (error) {
      this.doc = null;
      this.report = null;
      this.error = error instanceof Error ? error.message : "Failed to load synthesis document";
    }
  }

  async createSpace(title: string, projection: Projection) {
    try {
      this.error = null;
      const space = await runtime.createSpace({ title, projection });
      this.spaces = await runtime.listSpaces();
      this.activeSpaceId = space.id;
      this.focusedBlockId = null;
      this.doc = await runtime.readDoc(space.id);
      this.report = null;
      this.reportHistory = [];
      this.feed = [];
      this.newFeedEventIds = [];
      this.refreshSchedule = null;
      this.sourcePreview = null;
      this.sources = [];
      return space;
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to create space";
      throw error;
    }
  }

  async proposeEntity(role: EntityRole, text: string): Promise<ReviewItem> {
    if (!this.activeSpaceId) {
      const error = new Error("No active space");
      this.error = error.message;
      throw error;
    }
    try {
      this.error = null;
      return await runtime.proposeEntity(this.activeSpaceId, { role, text });
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to propose entity";
      throw error;
    }
  }

  async declareCondition(targetEntityId: string, terms: string): Promise<ReviewItem> {
    if (!this.activeSpaceId) throw new Error("No active space");
    try {
      this.error = null;
      return await runtime.declareCondition(this.activeSpaceId, { targetEntityId, terms });
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to declare condition";
      throw error;
    }
  }

  async editEntityText(entityId: string, text: string) {
    const previousVersion = this.report?.version ?? 0;
    try {
      this.error = null;
      this.doc = await runtime.editEntityText(entityId, { text });
      void this.waitForReportAfter(previousVersion, this.doc.spaceId);
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to edit entity";
      throw error;
    }
  }

  async changeEntityState(entityId: string, state: string) {
    const previousVersion = this.report?.version ?? 0;
    try {
      this.error = null;
      this.doc = await runtime.changeEntityState(entityId, { state });
      void this.waitForReportAfter(previousVersion, this.doc.spaceId);
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to update entity state";
      throw error;
    }
  }

  async submitInput(kind: InputKind, content: string) {
    if (!this.activeSpaceId) throw new Error("No active space");
    try {
      this.error = null;
      const anchor = kind === "comment" ? (this.inputAnchor ?? undefined) : undefined;
      const receipt = await runtime.submitInput(this.activeSpaceId, { kind, content, anchor });
      this.inputAnchor = null;
      this.sources = await runtime.readSources(this.activeSpaceId);
      return receipt;
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to submit input";
      throw error;
    }
  }

  async submitPublicObjection(input: PublicObjectionInput): Promise<ReviewItem> {
    if (!this.activeSpaceId) throw new Error("No active space");
    try {
      this.error = null;
      return await runtime.submitPublicObjection(this.activeSpaceId, input);
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to submit objection";
      throw error;
    }
  }

  beginComment(anchor: InputAnchor) {
    this.inputAnchor = anchor;
  }

  cancelComment() {
    this.inputAnchor = null;
  }

  async refreshSources(spaceId = this.activeSpaceId) {
    if (!spaceId) return;
    const sources = await runtime.readSources(spaceId);
    if (this.activeSpaceId === spaceId) this.sources = sources;
  }

  async previewSource(artifactId: string) {
    try {
      this.error = null;
      this.sourcePreview = await runtime.readArtifact(artifactId);
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to load source";
      throw error;
    }
  }

  async loadReportHistory() {
    if (!this.activeSpaceId) return;
    const spaceId = this.activeSpaceId;
    try {
      this.error = null;
      const history = await runtime.readReportHistory(spaceId);
      if (this.activeSpaceId === spaceId) this.reportHistory = history;
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to load report history";
      throw error;
    }
  }

  async loadReportVersionDiff(version: number): Promise<ReportVersionDiff | null> {
    if (!this.activeSpaceId) throw new Error("No active space");
    const spaceId = this.activeSpaceId;
    try {
      this.error = null;
      const diff = await runtime.readReportVersionDiff(spaceId, version);
      return this.activeSpaceId === spaceId ? diff : null;
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to load report diff";
      throw error;
    }
  }

  async loadFeed() {
    if (!this.activeSpaceId) return;
    try {
      this.error = null;
      const spaceId = this.activeSpaceId;
      const cursorKey = `fugue:space:${spaceId}:last-seen-event`;
      const previousCursor = typeof localStorage === "undefined" ? null : localStorage.getItem(cursorKey);
      const entries: FeedEntry[] = [];
      let after: string | undefined;
      for (;;) {
        const page = await runtime.readFeed(spaceId, after);
        entries.push(...page);
        if (page.length < 100) break;
        after = page.at(-1)?.eventId;
        if (!after) break;
      }
      if (this.activeSpaceId !== spaceId) return;
      const cursorIndex = previousCursor
        ? entries.findIndex((entry) => entry.eventId === previousCursor)
        : -1;
      this.feed = entries;
      this.newFeedEventIds = entries.slice(cursorIndex + 1).map((entry) => entry.eventId);
      const latest = entries.at(-1)?.eventId;
      if (latest && typeof localStorage !== "undefined") localStorage.setItem(cursorKey, latest);
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to load change feed";
      throw error;
    }
  }

  async putRefreshSchedule(cadenceSeconds: number, enabled: boolean) {
    if (!this.activeSpaceId) throw new Error("No active space");
    try {
      this.error = null;
      this.refreshSchedule = await runtime.putRefreshSchedule(this.activeSpaceId, {
        cadenceSeconds,
        enabled
      });
      return this.refreshSchedule;
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to update refresh cadence";
      throw error;
    }
  }

  async waitForReportAfter(previousVersion: number, spaceId = this.activeSpaceId) {
    if (!spaceId) return;
    const startedAt = Date.now();
    while (Date.now() - startedAt < 240_000) {
      try {
        const report = await runtime.readReport(spaceId);
        if (report && report.version > previousVersion) {
          if (this.activeSpaceId === spaceId) {
            this.report = report;
            this.reportHistory = [];
          }
          return report;
        }
      } catch {
        // Retry transient read failures while the worker is running.
      }
      await new Promise((resolve) => window.setTimeout(resolve, 1500));
    }
  }

  async synthesize(mode: "full" | "selective", instruction?: string) {
    if (!this.activeSpaceId || this.synthesizing) return;
    const spaceId = this.activeSpaceId;
    const previousVersion = this.report?.version ?? 0;
    this.synthesizing = true;
    this.error = null;
    try {
      if (mode === "selective") {
        await runtime.synthesize(spaceId, { mode, instruction: instruction?.trim() ?? "" });
      } else {
        await runtime.synthesize(spaceId, { mode });
      }
      const startedAt = Date.now();
      while (Date.now() - startedAt < 240_000) {
        const report = await runtime.readReport(spaceId);
        if (report && report.version > previousVersion) {
          if (this.activeSpaceId === spaceId) {
            this.report = report;
            this.reportHistory = [];
          }
          return report;
        }
        await new Promise((resolve) => window.setTimeout(resolve, 1500));
      }
      throw new Error("Synthesis did not finish in time");
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to synthesize report";
      throw error;
    } finally {
      this.synthesizing = false;
    }
  }

  focusBlock(id: string | null) {
    this.focusedBlockId = id;
  }

  /** Insert a commit mark after the given block once its review item is accepted. */
  insertCommitMark(afterBlockId: string, summary: string) {
    if (!this.doc) return;
    const version = this.doc.version + 1;
    const mark: DocNode = {
      kind: "commit",
      id: `n-commit-${version}`,
      label: `v${version}`,
      author: "you",
      summary
    };
    const index = this.doc.nodes.findIndex((node) => node.id === afterBlockId);
    const nodes = [...this.doc.nodes];
    nodes.splice(index === -1 ? 0 : index + 1, 0, mark);
    this.doc = { ...this.doc, version, nodes };
  }

  replaceDoc(doc: SynthesisDoc) {
    this.activeSpaceId = doc.spaceId;
    this.doc = doc;
  }
}

export const workspace = new WorkspaceState();
