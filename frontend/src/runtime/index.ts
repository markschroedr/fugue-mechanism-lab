import { createHttpRuntime } from "./http";
import type { AppRuntime } from "./types";

export const runtime: AppRuntime = createHttpRuntime();
export type {
  AppRuntime,
  CostSummary,
  HealthResponse,
  LabExperiment,
  LabFile,
  LabFileSnapshot,
  BoardPost,
  OpenSessionInput,
  RuntimeKind,
  SocietyAdminAgentDetail,
  SocietyAdminAgentSummary,
  SocietyAdminLedgerEntry,
  SocietyAdminOverview,
  SocietyAdminTranscriptEntry,
  SocietyAdminWakeReceipt
} from "./types";
