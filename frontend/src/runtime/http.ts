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

export function createHttpRuntime(): AppRuntime {
  return {
    kind: "http",
    async health(): Promise<HealthResponse> {
      const health = await request<Omit<HealthResponse, "runtime">>("/api/health");
      return {
        ...health,
        runtime: "http"
      };
    },
    readCosts(): Promise<CostSummary> {
      return request("/api/costs");
    },
    listLabExperiments(): Promise<LabExperiment[] | null> {
      return requestOrNullOnNotFound("/api/lab/experiments");
    },
    readLabExperimentOverview(experimentId: string): Promise<SocietyAdminOverview> {
      return request(`${labExperimentPath(experimentId)}/overview`);
    },
    readLabExperimentAgent(experimentId: string, address: string): Promise<SocietyAdminAgentDetail> {
      return request(`${labExperimentPath(experimentId)}/agents/${encodeURIComponent(address)}`);
    },
    listLabExperimentAgentFiles(experimentId: string, address: string): Promise<LabFileSnapshot> {
      return request(`${labExperimentPath(experimentId)}/agents/${encodeURIComponent(address)}/files`);
    },
    readLabExperimentAgentTextFile(experimentId: string, address: string, path: string): Promise<string> {
      return requestText(labAgentFilePath(experimentId, address, path));
    },
    labExperimentAgentFileUrl(experimentId: string, address: string, path: string, download = false): string {
      const url = labAgentFilePath(experimentId, address, path);
      return download ? `${url}?download=1` : url;
    },
    readSocietyAdminOverview(): Promise<SocietyAdminOverview> {
      return request("/api/society/admin");
    },
    readSocietyAdminAgent(address: string): Promise<SocietyAdminAgentDetail> {
      return request(`/api/society/admin/agents/${encodeURIComponent(address)}`);
    },
    authSession(): Promise<AuthActor | null> {
      return request("/api/auth/session");
    },
    register(input: AuthInput): Promise<AuthActor> {
      return request("/api/auth/register", { method: "POST", body: JSON.stringify(input) });
    },
    login(input: AuthInput): Promise<AuthActor> {
      return request("/api/auth/login", { method: "POST", body: JSON.stringify(input) });
    },
    async logout(): Promise<void> {
      await requestEmpty("/api/auth/logout", { method: "POST" });
    },
    listSpaces(): Promise<SpaceRef[]> {
      return request("/api/spaces");
    },
    readDoc(spaceId: string): Promise<SynthesisDoc> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/doc`);
    },
    readReport(spaceId: string): Promise<SynthesisReport | null> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/report`);
    },
    readReportHistory(spaceId: string): Promise<ReportVersion[]> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/report/history`);
    },
    readReportVersionDiff(spaceId: string, version: number): Promise<ReportVersionDiff> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/report/versions/${version}/diff`);
    },
    readFeed(spaceId: string, after?: string): Promise<FeedEntry[]> {
      const query = after ? `?after=${encodeURIComponent(after)}` : "";
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/feed${query}`);
    },
    readRefreshSchedule(spaceId: string): Promise<RefreshSchedule | null> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/refresh-schedule`);
    },
    readFalsifiers(spaceId: string): Promise<FalsifierGroup[]> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/falsifiers`);
    },
    readSources(spaceId: string): Promise<ImportedSource[]> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/sources`);
    },
    readArtifact(artifactId: string): Promise<ArtifactReadModel> {
      return request(`/api/artifacts/${encodeURIComponent(artifactId)}`);
    },
    createSpace(input: CreateSpaceInput): Promise<SpaceRef> {
      return request("/api/spaces", {
        method: "POST",
        body: JSON.stringify(input)
      });
    },
    proposeEntity(spaceId: string, input: CreateEntityInput): Promise<ReviewItem> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/entities`, {
        method: "POST",
        body: JSON.stringify(input)
      });
    },
    declareCondition(spaceId: string, input: DeclareConditionInput): Promise<ReviewItem> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/conditions`, {
        method: "POST",
        body: JSON.stringify(input)
      });
    },
    editEntityText(entityId: string, input: EditEntityTextInput): Promise<SynthesisDoc> {
      return request(`/api/entities/${encodeURIComponent(entityId)}`, {
        method: "PATCH",
        body: JSON.stringify(input)
      });
    },
    changeEntityState(entityId: string, input: ChangeEntityStateInput): Promise<SynthesisDoc> {
      return request(`/api/entities/${encodeURIComponent(entityId)}/state`, {
        method: "POST",
        body: JSON.stringify(input)
      });
    },
    submitInput(spaceId: string, input: SubmitInput): Promise<InputReceipt> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/input`, {
        method: "POST",
        body: JSON.stringify(input)
      });
    },
    submitPublicObjection(spaceId: string, input: PublicObjectionInput): Promise<ReviewItem> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/objections`, {
        method: "POST",
        body: JSON.stringify(input)
      });
    },
    synthesize(spaceId: string, input: SynthesizeInput): Promise<WorkerJobReceipt> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/synthesis`, {
        method: "POST",
        body: JSON.stringify(input)
      });
    },
    putRefreshSchedule(spaceId: string, input: PutRefreshScheduleInput): Promise<RefreshSchedule> {
      return request(`/api/spaces/${encodeURIComponent(spaceId)}/refresh-schedule`, {
        method: "PUT",
        body: JSON.stringify(input)
      });
    },
    listReview(): Promise<ReviewItem[]> {
      return request("/api/review");
    },
    acceptReviewItem(reviewId: string): Promise<SynthesisDoc> {
      return request(`/api/review/${encodeURIComponent(reviewId)}/accept`, { method: "POST" });
    },
    rejectReviewItem(reviewId: string): Promise<void> {
      return requestEmpty(`/api/review/${encodeURIComponent(reviewId)}/reject`, { method: "POST" });
    },
    openSession(input: OpenSessionInput): Promise<SessionReadModel> {
      return request("/api/sessions", {
        method: "POST",
        body: JSON.stringify(input)
      });
    },
    readSession(threadId: string): Promise<SessionReadModel> {
      return request(`/api/sessions/${encodeURIComponent(threadId)}`);
    },
    appendUserMessage(threadId: string, text: string): Promise<SessionReadModel> {
      return request(`/api/sessions/${encodeURIComponent(threadId)}/messages`, {
        method: "POST",
        body: JSON.stringify({ text })
      });
    },
    discardSession(threadId: string): Promise<SessionReadModel> {
      return request(`/api/sessions/${encodeURIComponent(threadId)}/discard`, { method: "POST" });
    },
    commitSession(threadId: string): Promise<ReviewItem> {
      return request(`/api/sessions/${encodeURIComponent(threadId)}/commit`, { method: "POST" });
    }
  };
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: {
      "content-type": "application/json",
      ...init.headers
    }
  });

  if (!response.ok) {
    throw new Error(await errorMessage(response));
  }

  return (await response.json()) as T;
}

async function requestOrNullOnNotFound<T>(path: string): Promise<T | null> {
  const response = await fetch(path, { headers: { "content-type": "application/json" } });
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(await errorMessage(response));
  return (await response.json()) as T;
}

async function requestText(path: string): Promise<string> {
  const response = await fetch(path);
  if (!response.ok) throw new Error(await errorMessage(response));
  return response.text();
}

function labExperimentPath(experimentId: string): string {
  return `/api/lab/experiments/${encodeURIComponent(experimentId)}`;
}

function labAgentFilePath(experimentId: string, address: string, path: string): string {
  return `${labExperimentPath(experimentId)}/agents/${encodeURIComponent(address)}/files/${encodeURIComponent(path)}`;
}

async function requestEmpty(path: string, init: RequestInit): Promise<void> {
  const response = await fetch(path, init);
  if (!response.ok) throw new Error(await errorMessage(response));
}

async function errorMessage(response: Response): Promise<string> {
  try {
    const payload = (await response.json()) as { error?: string };
    return payload.error ?? `Request failed with ${response.status}`;
  } catch {
    return `Request failed with ${response.status}`;
  }
}
