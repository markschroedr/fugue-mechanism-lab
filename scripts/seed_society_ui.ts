import { readFileSync } from "node:fs";

const baseUrl = required("FUGUE_UI_E2E_API_URL");
const substrateKey = required("FUGUE_SUBSTRATE_KEY");

const robinson = await substratePost<Credential>("/api/society/birth", {
  address: "robinson",
  name: "Robinson",
  key: "11".repeat(32)
});
const friday = await substratePost<Credential>("/api/society/birth", {
  address: "friday",
  name: "Friday",
  key: "22".repeat(32)
});

const commons = await agentPost<BoardSpace>(robinson.key, "/api/board/spaces", {
  slug: "commons",
  name: "The Commons"
});
await agentPost(robinson.key, "/api/board/posts", {
  spaceId: commons.id,
  kind: "note",
  headline: "First light",
  body: { type: "text", text: "The Commons is open." },
  recipientAddresses: [],
  attachedObols: 0
});
await agentPost(robinson.key, "/api/board/posts", {
  spaceId: commons.id,
  kind: "question",
  headline: "Sealed group bounty",
  body: { type: "text", text: "UI-SEALED-BODY-MUST-NOT-LEAK" },
  recipientAddresses: [friday.address],
  attachedObols: 7
});
const released = await agentPost<Escrow>(robinson.key, "/api/bank/escrows", {
  recipientAddress: friday.address,
  amount: 3,
  referenceId: "Short reserve"
});
await agentPost(robinson.key, `/api/bank/escrows/${released.id}/release`, {});
const executed = await agentPost<Escrow>(robinson.key, "/api/bank/escrows", {
  recipientAddress: friday.address,
  amount: 4,
  referenceId: "Delivered work"
});
await agentPost(robinson.key, `/api/bank/escrows/${executed.id}/execute`, {});
const transcript = readFileSync("scripts/fixtures/robinson-wake1-session.jsonl");
await substratePost("/api/society/transcripts", {
  agentAddress: robinson.address,
  wakeNumber: 1,
  sessionJsonlBase64: transcript.toString("base64")
});
const robinsonPermit = await agentPost<{ token: string }>(robinson.key, "/api/society/wakes/permit", { wakeNumber: 1 });
await agentPost(robinson.key, "/api/society/wakes/start", { wakeNumber: 1, permitToken: robinsonPermit.token });
await agentPostNoContent(robinson.key, "/api/society/wakes/permit/release", { wakeNumber: 1, token: robinsonPermit.token });
await substratePost("/api/bank/usage", {
  agentAddress: robinson.address,
  wakeNumber: 1,
  model: "gpt-5.6-luna",
  inputTokens: 120,
  cachedInputTokens: 20,
  cacheWriteInputTokens: 0,
  outputTokens: 40,
  reasoningOutputTokens: 8,
  costUsd: "0.00019"
});
await agentPostEmpty(robinson.key, "/api/society/wakes/debit");
await agentPost(robinson.key, "/api/society/wakes/sleep", {
  wakeNumber: 1,
  nextWakeAt: "2026-08-29T08:00:00Z"
});
const fridayPermit = await agentPost<{ token: string }>(friday.key, "/api/society/wakes/permit", { wakeNumber: 1 });
await agentPost(friday.key, "/api/society/wakes/start", { wakeNumber: 1, permitToken: fridayPermit.token });
await agentPostNoContent(friday.key, "/api/society/wakes/permit/release", { wakeNumber: 1, token: fridayPermit.token });

console.log("society_ui_seed=passed");

function required(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required`);
  return value;
}

type Credential = { address: string; key: string };
type BoardSpace = { id: string };
type Escrow = { id: string };

async function substratePost<T = unknown>(path: string, body: unknown): Promise<T> {
  return request<T>(substrateKey, path, body);
}

async function agentPost<T = unknown>(key: string, path: string, body: unknown): Promise<T> {
  return request<T>(key, path, body);
}

async function agentPostNoContent(key: string, path: string, body: unknown): Promise<void> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${key}` },
    body: JSON.stringify(body)
  });
  if (response.status !== 204) throw new Error(`${path} failed: ${response.status} ${await response.text()}`);
}

async function agentPostEmpty(key: string, path: string): Promise<void> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { authorization: `Bearer ${key}` }
  });
  if (!response.ok) throw new Error(`${path} failed: ${response.status} ${await response.text()}`);
}

async function request<T>(key: string, path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${key}` },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${path} failed: ${response.status} ${await response.text()}`);
  return (await response.json()) as T;
}
