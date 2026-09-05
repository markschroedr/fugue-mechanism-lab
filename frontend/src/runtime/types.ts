import type {
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
  HealthResponse as ContractHealthResponse,
  ImportedSource,
  InputReceipt,
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
  SynthesisDoc,
  SocietyAdminAgentDetail,
  SocietyAdminOverview,
  SynthesisReport,
  SynthesizeInput,
  WorkerJobReceipt
} from "./generated";

export type * from "./generated";

export type RuntimeKind = "http" | "tauri";

export type LabExperiment = {
  id: string;
  state: "paused" | "prepared" | "active" | "failed";
  apiReachable: boolean;
  apiPort: number;
};

export type LabFile = {
  path: string;
  byteLength: number;
  kind: "text" | "binary";
};

export type LabFileSnapshot = {
  snapshotAt: string | null;
  files: LabFile[];
};

/** Runtime identity is added by the handwritten adapter, not sent over the wire. */
export type HealthResponse = ContractHealthResponse & {
  runtime: RuntimeKind;
};

export interface AppRuntime {
  readonly kind: RuntimeKind;
  health(): Promise<HealthResponse>;
  readCosts(): Promise<CostSummary>;
  listLabExperiments(): Promise<LabExperiment[] | null>;
  readLabExperimentOverview(experimentId: string): Promise<SocietyAdminOverview>;
  readLabExperimentAgent(experimentId: string, address: string): Promise<SocietyAdminAgentDetail>;
  listLabExperimentAgentFiles(experimentId: string, address: string): Promise<LabFileSnapshot>;
  readLabExperimentAgentTextFile(experimentId: string, address: string, path: string): Promise<string>;
  labExperimentAgentFileUrl(experimentId: string, address: string, path: string, download?: boolean): string;
  readSocietyAdminOverview(): Promise<SocietyAdminOverview>;
  readSocietyAdminAgent(address: string): Promise<SocietyAdminAgentDetail>;
  authSession(): Promise<AuthActor | null>;
  register(input: AuthInput): Promise<AuthActor>;
  login(input: AuthInput): Promise<AuthActor>;
  logout(): Promise<void>;
  listSpaces(): Promise<SpaceRef[]>;
  readDoc(spaceId: string): Promise<SynthesisDoc>;
  readReport(spaceId: string): Promise<SynthesisReport | null>;
  readReportHistory(spaceId: string): Promise<ReportVersion[]>;
  readReportVersionDiff(spaceId: string, version: number): Promise<ReportVersionDiff>;
  readFeed(spaceId: string, after?: string): Promise<FeedEntry[]>;
  readRefreshSchedule(spaceId: string): Promise<RefreshSchedule | null>;
  readFalsifiers(spaceId: string): Promise<FalsifierGroup[]>;
  readSources(spaceId: string): Promise<ImportedSource[]>;
  readArtifact(artifactId: string): Promise<ArtifactReadModel>;
  createSpace(input: CreateSpaceInput): Promise<SpaceRef>;
  proposeEntity(spaceId: string, input: CreateEntityInput): Promise<ReviewItem>;
  declareCondition(spaceId: string, input: DeclareConditionInput): Promise<ReviewItem>;
  editEntityText(entityId: string, input: EditEntityTextInput): Promise<SynthesisDoc>;
  changeEntityState(entityId: string, input: ChangeEntityStateInput): Promise<SynthesisDoc>;
  submitInput(spaceId: string, input: SubmitInput): Promise<InputReceipt>;
  submitPublicObjection(spaceId: string, input: PublicObjectionInput): Promise<ReviewItem>;
  synthesize(spaceId: string, input: SynthesizeInput): Promise<WorkerJobReceipt>;
  putRefreshSchedule(spaceId: string, input: PutRefreshScheduleInput): Promise<RefreshSchedule>;
  listReview(): Promise<ReviewItem[]>;
  acceptReviewItem(reviewId: string): Promise<SynthesisDoc>;
  rejectReviewItem(reviewId: string): Promise<void>;
  openSession(input: OpenSessionInput): Promise<SessionReadModel>;
  readSession(threadId: string): Promise<SessionReadModel>;
  appendUserMessage(threadId: string, text: string): Promise<SessionReadModel>;
  discardSession(threadId: string): Promise<SessionReadModel>;
  commitSession(threadId: string): Promise<ReviewItem>;
}
