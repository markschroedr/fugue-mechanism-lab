import type {
  AppRuntime,
  ArtifactReadModel,
  AuthActor,
  AuthInput,
  ChangeEntityStateInput,
  CostSummary,
  CreateEntityInput,
  CreateSpaceInput,
  DeclareConditionInput,
  EditEntityTextInput,
  FeedEntry,
  FalsifierGroup,
  HealthResponse,
  ImportedSource,
  InputReceipt,
  LabExperiment,
  LabFileSnapshot,
  OpenSessionInput,
  PublicObjectionInput,
  PutRefreshScheduleInput,
  RefreshSchedule,
  ReportVersion,
  ReportVersionDiff,
  ReviewItem,
  SessionReadModel,
  SpaceRef,
  SubmitInput,
  SocietyAdminAgentDetail,
  SocietyAdminOverview,
  SynthesisDoc,
  SynthesisReport,
  SynthesizeInput,
  WorkerJobReceipt
} from "./types";

type TauriInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export function createTauriRuntime(invoke: TauriInvoke): AppRuntime {
  return {
    kind: "tauri",
    async health(): Promise<HealthResponse> {
      const health = await invoke<Omit<HealthResponse, "runtime">>("health");
      return {
        ...health,
        runtime: "tauri"
      };
    },
    readCosts(): Promise<CostSummary> {
      return invoke("read_costs");
    },
    async listLabExperiments(): Promise<LabExperiment[] | null> {
      return null;
    },
    readLabExperimentOverview(_experimentId: string): Promise<SocietyAdminOverview> {
      return invoke("read_society_admin_overview");
    },
    readLabExperimentAgent(_experimentId: string, address: string): Promise<SocietyAdminAgentDetail> {
      return invoke("read_society_admin_agent", { address });
    },
    listLabExperimentAgentFiles(_experimentId: string, _address: string): Promise<LabFileSnapshot> {
      return Promise.reject(new Error("Experiment snapshots are not available in the desktop runtime"));
    },
    readLabExperimentAgentTextFile(_experimentId: string, _address: string, _path: string): Promise<string> {
      return Promise.reject(new Error("Experiment snapshots are not available in the desktop runtime"));
    },
    labExperimentAgentFileUrl(_experimentId: string, _address: string, _path: string): string {
      return "";
    },
    readSocietyAdminOverview(): Promise<SocietyAdminOverview> {
      return invoke("read_society_admin_overview");
    },
    readSocietyAdminAgent(address: string): Promise<SocietyAdminAgentDetail> {
      return invoke("read_society_admin_agent", { address });
    },
    authSession(): Promise<AuthActor | null> {
      return invoke("auth_session");
    },
    register(input: AuthInput): Promise<AuthActor> {
      return invoke("register", { input });
    },
    login(input: AuthInput): Promise<AuthActor> {
      return invoke("login", { input });
    },
    logout(): Promise<void> {
      return invoke("logout");
    },
    listSpaces(): Promise<SpaceRef[]> {
      return invoke("list_spaces");
    },
    readDoc(spaceId: string): Promise<SynthesisDoc> {
      return invoke("read_doc", { spaceId });
    },
    readReport(spaceId: string): Promise<SynthesisReport | null> {
      return invoke("read_report", { spaceId });
    },
    readReportHistory(spaceId: string): Promise<ReportVersion[]> {
      return invoke("read_report_history", { spaceId });
    },
    readReportVersionDiff(spaceId: string, version: number): Promise<ReportVersionDiff> {
      return invoke("read_report_version_diff", { spaceId, version });
    },
    readFeed(spaceId: string, after?: string): Promise<FeedEntry[]> {
      return invoke("read_feed", { spaceId, after });
    },
    readRefreshSchedule(spaceId: string): Promise<RefreshSchedule | null> {
      return invoke("read_refresh_schedule", { spaceId });
    },
    readFalsifiers(spaceId: string): Promise<FalsifierGroup[]> {
      return invoke("read_falsifiers", { spaceId });
    },
    readSources(spaceId: string): Promise<ImportedSource[]> {
      return invoke("read_sources", { spaceId });
    },
    readArtifact(artifactId: string): Promise<ArtifactReadModel> {
      return invoke("read_artifact", { artifactId });
    },
    createSpace(input: CreateSpaceInput): Promise<SpaceRef> {
      return invoke("create_space", { input });
    },
    proposeEntity(spaceId: string, input: CreateEntityInput): Promise<ReviewItem> {
      return invoke("propose_entity", { spaceId, input });
    },
    declareCondition(spaceId: string, input: DeclareConditionInput): Promise<ReviewItem> {
      return invoke("declare_condition", { spaceId, input });
    },
    editEntityText(entityId: string, input: EditEntityTextInput): Promise<SynthesisDoc> {
      return invoke("edit_entity_text", { entityId, input });
    },
    changeEntityState(entityId: string, input: ChangeEntityStateInput): Promise<SynthesisDoc> {
      return invoke("change_entity_state", { entityId, input });
    },
    submitInput(spaceId: string, input: SubmitInput): Promise<InputReceipt> {
      return invoke("submit_input", { spaceId, input });
    },
    submitPublicObjection(spaceId: string, input: PublicObjectionInput): Promise<ReviewItem> {
      return invoke("submit_public_objection", { spaceId, input });
    },
    synthesize(spaceId: string, input: SynthesizeInput): Promise<WorkerJobReceipt> {
      return invoke("synthesize", { spaceId, input });
    },
    putRefreshSchedule(spaceId: string, input: PutRefreshScheduleInput): Promise<RefreshSchedule> {
      return invoke("put_refresh_schedule", { spaceId, input });
    },
    listReview(): Promise<ReviewItem[]> {
      return invoke("list_review");
    },
    acceptReviewItem(reviewId: string): Promise<SynthesisDoc> {
      return invoke("accept_review_item", { reviewId });
    },
    rejectReviewItem(reviewId: string): Promise<void> {
      return invoke("reject_review_item", { reviewId });
    },
    openSession(input: OpenSessionInput): Promise<SessionReadModel> {
      return invoke("open_session", { input });
    },
    readSession(threadId: string): Promise<SessionReadModel> {
      return invoke("read_session", { threadId });
    },
    appendUserMessage(threadId: string, text: string): Promise<SessionReadModel> {
      return invoke("append_user_message", { threadId, text });
    },
    discardSession(threadId: string): Promise<SessionReadModel> {
      return invoke("discard_session", { threadId });
    },
    commitSession(threadId: string): Promise<ReviewItem> {
      return invoke("commit_session", { threadId });
    }
  };
}
