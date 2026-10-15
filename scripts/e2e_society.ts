import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { printScenarioCostAtTeardown } from "./lib/scenario_cost";

const port = "49419";
const dataDir = mkdtempSync(join(tmpdir(), "fugue-society-"));
const baseUrl = `http://127.0.0.1:${port}`;
const substrateKey = randomBytes(32).toString("hex");
let operatorCookie = "";
let apiServerOutput = "";
const apiServer = startApi();

type AgentCredential = { actorId: string; address: string; key: string };
type BankSnapshot = {
  totalMinted: number;
  totalBalances: number;
  openEscrows: number;
  accounts: Array<{ address: string; balance: number }>;
};
type Escrow = { id: string; status: string; amount: number; resolutionAuthority: "payer" | "recipient_read" };
type Space = { id: string; slug: string; name: string };
type PostBody =
  | { type: "text"; text: string }
  | { type: "link"; url: string }
  | { type: "file"; filename: string; mediaType: string; contentBase64: string };
type Post = {
  id: string;
  sequence: number;
  reference: string;
  kind: string;
  headline: string;
  sealed: boolean;
  body?: PostBody;
  parentPostId?: string;
  parentReference?: string;
  attachedObols: number;
};
type Feed = { cursor?: number; posts: Post[] };
type ReadReceipt = { postId: string; post: Post; cursor: number; settledObols: number };
type Identity = { address: string; name?: string; description?: string; balance: number };
type UsageRecord = {
  id: string;
  agentAddress: string;
  model: string;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  costUsd?: number;
};
try {
  await waitForHealth();
  const invalidNode = spawnSync("python3", ["scripts/society_node.py", "post"], { encoding: "utf8" });
  assert(invalidNode.status === 1 && JSON.parse(invalidNode.stderr).error.includes("required"), "CLI argument errors should be compact JSON");
  operatorCookie = await registerOperator();

  const alice = await createAgent("alice");
  const bob = await createAgent("bob");
  const carol = await createAgent("carol");

  await expectStatus("POST", "/api/directory/register", { name: "Nobody" }, 401);
  await expectStatus("POST", "/api/directory/register", { name: "Nobody" }, 401, "not-a-valid-key");
  await expectStatus("POST", "/api/bank/transfers", { recipientAddress: "bob", amount: 1 }, 401);
  await agentPost(alice.key, "/api/directory/register", { name: "Alice", description: "Finds useful work" });
  await agentPost(bob.key, "/api/directory/register", { name: "Bob", description: "Reads carefully" });
  await agentPost(carol.key, "/api/directory/register", { name: "Carol" });
  const directory = await get<Array<{ address: string; name: string; description?: string }>>("/api/directory");
  assert(directory.map((entry) => entry.address).join(",") === "alice,bob,carol", "directory should list registered agents by address");
  assert(directory[0].description === "Finds useful work", "directory should round-trip the optional offer");

  await expectStatus("POST", "/api/bank/mint", { recipientAddress: "alice", amount: 100 }, 401, undefined, true);
  await substratePost("/api/bank/mint", { recipientAddress: "alice", amount: 100 });
  await assertConservation(100, 100, 0);
  await expectStatus("POST", "/api/bank/mint", { recipientAddress: "alice", amount: -1 }, 400, substrateKey);
  await expectStatus("POST", "/api/bank/mint", { recipientAddress: "alice", amount: 9_007_199_254_740_991 }, 400, substrateKey);
  await expectStatus("POST", "/api/bank/transfers", { recipientAddress: "bob", amount: 0 }, 400, alice.key);
  await expectStatus("POST", "/api/bank/transfers", { recipientAddress: "bob", amount: -1 }, 400, alice.key);
  await expectStatus("POST", "/api/bank/transfers", { recipientAddress: "bob", amount: 101 }, 400, alice.key);

  await agentPost(alice.key, "/api/bank/transfers", { actorId: carol.actorId, recipientAddress: "bob", amount: 10 });
  let bank = await assertConservation(100, 100, 0);
  assert(balance(bank, "alice") === 90 && balance(bank, "bob") === 10, "transfer must debit the key holder, never a body actor id");

  const released = await agentPost<Escrow>(alice.key, "/api/bank/escrows", { recipientAddress: "bob", amount: 20 });
  await assertConservation(100, 80, 20);
  assert(released.resolutionAuthority === "payer", "plain escrow should record payer resolution authority");
  await expectStatus("POST", `/api/bank/escrows/${released.id}/execute`, {}, 400, bob.key);
  const releasedResult = await agentPost<Escrow>(alice.key, `/api/bank/escrows/${released.id}/release`, {});
  assert(releasedResult.status === "released", "escrow release should return funds to its creator");
  await assertConservation(100, 100, 0);

  const executed = await agentPost<Escrow>(alice.key, "/api/bank/escrows", { recipientAddress: "bob", amount: 15 });
  await assertConservation(100, 85, 15);
  const executedResult = await agentPost<Escrow>(alice.key, `/api/bank/escrows/${executed.id}/execute`, {});
  assert(executedResult.status === "executed", "escrow execute should pay its recipient");
  bank = await assertConservation(100, 100, 0);
  assert(balance(bank, "alice") === 75 && balance(bank, "bob") === 25, "escrow execute should move obols to its recipient");

  const space = await agentPost<Space>(alice.key, "/api/board/spaces", { slug: "commons", name: "Commons" });
  assert(!(await get<Array<{ id: string }>>("/api/spaces")).some((item) => item.id === space.id), "human space listing should exclude society spaces");
  assert(sqlite("operations.sqlite", "select count(*) from sqlite_master where type = 'table' and name = 'society_channel';") === "0", "the society_channel table must not exist");
  await expectStatus("POST", "/api/board/posts", { spaceId: space.id, kind: "note", headline: "No", body: { type: "text", text: "unauthorized" }, recipientAddresses: [], attachedObols: 0 }, 401);
  await expectStatus("POST", "/api/board/posts", { spaceId: space.id, kind: "note", headline: "Malformed file", body: { type: "file", filename: "bad.bin", mediaType: "application/octet-stream", contentBase64: "not base64!" }, recipientAddresses: ["bob"], attachedObols: 1 }, 400, alice.key);
  const unpaid = await agentPost<Post>(alice.key, "/api/board/posts", {
    actorId: carol.actorId,
    spaceId: space.id,
    kind: "note",
    headline: "Unpaid hello",
    body: { type: "text", text: "An unpaid hello" },
    recipientAddresses: ["bob"],
    attachedObols: 0
  });
  assert(unpaid.authorAddress === "alice", "board author must derive from the key, never a body actor id");
  const paid = await agentPost<Post>(alice.key, "/api/board/posts", {
    spaceId: space.id,
    kind: "finding",
    headline: "Paid finding",
    body: { type: "text", text: "A paid finding" },
    recipientAddresses: ["bob"],
    attachedObols: 25
  });
  const group = await agentPost<Post>(alice.key, "/api/board/posts", {
    spaceId: space.id,
    kind: "question",
    headline: "Group question",
    body: { type: "text", text: "A group question" },
    parentPostId: unpaid.id,
    recipientAddresses: ["bob", "carol"],
    attachedObols: 0
  });
  await assertConservation(100, 75, 25);
  const attentionEscrowId = sqlite("operations.sqlite", `select escrow_id from society_attention where post_id = '${paid.id.replaceAll("'", "''")}';`);
  const paidArtifactId = sqlite("operations.sqlite", `select artifact_id from society_post where id = '${paid.id.replaceAll("'", "''")}';`);
  await expectGetStatus(`/api/artifacts/${paidArtifactId}`, 404);
  assert(sqlite("operations.sqlite", `select resolution_authority from obol_ledger where escrow_id = '${attentionEscrowId.replaceAll("'", "''")}' and kind = 'escrow_create';`) === "recipient_read", "attention escrow should record recipient-read resolution authority");
  await expectStatus("POST", `/api/bank/escrows/${attentionEscrowId}/release`, {}, 400, alice.key);
  await expectStatus("POST", `/api/bank/escrows/${attentionEscrowId}/execute`, {}, 400, alice.key);

  const bobInbox = await agentGet<Post[]>(bob.key, "/api/board/inbox");
  assert(bobInbox.map((post) => post.id).join(",") === [paid.id, group.id, unpaid.id].join(","), "direct inbox items should rank by attached Obols and then newest first");
  const sealedInboxPost = bobInbox[0];
  assert(sealedInboxPost.headline === "Paid finding" && sealedInboxPost.sealed && sealedInboxPost.body === undefined, "recipient inbox should show a paid post headline but withhold its body");
  const carolInbox = await agentGet<Post[]>(carol.key, "/api/board/inbox");
  assert(carolInbox.some((post) => post.id === group.id), "group addressing should deliver to every addressed agent");

  const initialFeed = await agentGet<Feed>(bob.key, `/api/board/spaces/${space.id}/posts`);
  assert(initialFeed.posts.map((post) => post.kind).join(",") === "note,finding,question", "post kinds should round-trip through the space feed");
  assert(initialFeed.posts.find((post) => post.id === paid.id)?.body === undefined, "space feed must not leak a sealed body");
  assert((initialFeed.posts.find((post) => post.id === unpaid.id)?.body as { text: string }).text === "An unpaid hello", "unpaid posts should never be sealed");
  assert(initialFeed.posts.find((post) => post.id === group.id)?.parentPostId === unpaid.id, "thread replies should retain their parent");
  assert(unpaid.reference === `P${unpaid.sequence}` && group.parentReference === unpaid.reference, "posts should expose stable short references for replies");
  const shownPost = societyAdmin("show", unpaid.reference);
  assert(shownPost.includes(`${unpaid.reference} `) && shownPost.includes("Unpaid hello") && shownPost.includes("An unpaid hello"), "admin CLI should show one historical post by short reference");
  const shownRange = societyAdmin("show", `${unpaid.reference}-${group.reference}`);
  assert(shownRange.includes("Unpaid hello") && shownRange.includes("Paid finding") && shownRange.includes("Group question"), "admin CLI should show an inclusive historical post range");

  const paidRead = await agentPost<ReadReceipt>(bob.key, `/api/board/posts/${paid.reference}/read`, {});
  assert(paidRead.settledObols === 25 && !paidRead.post.sealed && (paidRead.post.body as { text: string }).text === "A paid finding", "the settling read should reveal the paid body and settle its obols atomically");
  bank = await assertConservation(100, 100, 0);
  assert(balance(bank, "bob") === 50, "settled attention should credit the reader");
  const afterPaid = await agentGet<Feed>(bob.key, `/api/board/spaces/${space.id}/posts`);
  assert(afterPaid.cursor === paid.sequence && afterPaid.posts.map((post) => post.id).join(",") === group.id, "feed should return posts newer than the agent read cursor");

  const groupRead = await agentPost<ReadReceipt>(bob.key, `/api/board/posts/${group.id}/read`, {});
  assert(groupRead.cursor === group.sequence && groupRead.settledObols === 0, "unpaid read should still advance the cursor");
  const noNewPosts = await agentGet<Feed>(bob.key, `/api/board/spaces/${space.id}/posts`);
  assert(noNewPosts.posts.length === 0, "read cursor should make new-since empty after the newest read");
  const secondPaidRead = await agentPost<ReadReceipt>(bob.key, `/api/board/posts/${paid.id}/read`, {});
  assert(secondPaidRead.settledObols === 0, "repeat reads must not settle twice");
  await assertConservation(100, 100, 0);

  const wakePermit = await agentPost<{ token: string }>(alice.key, "/api/society/wakes/permit", { wakeNumber: 1 });
  await expectStatus("POST", "/api/society/wakes/start", { wakeNumber: 1, permitToken: "wrong" }, 401, alice.key);
  const wakeStart = await agentPost<{ lifecycle: { phase: string; wakeNumber: number } }>(alice.key, "/api/society/wakes/start", { wakeNumber: 1, permitToken: wakePermit.token });
  assert(wakeStart.lifecycle.phase === "awake" && wakeStart.lifecycle.wakeNumber === 1, "runner lifecycle should record an explicitly permitted awake state");
  await agentPostNoContent(alice.key, "/api/society/wakes/permit/release", { wakeNumber: 1, token: wakePermit.token });
  const sleeping = await agentPost<{ phase: string; nextWakeAt?: string }>(alice.key, "/api/society/wakes/sleep", { wakeNumber: 1, nextWakeAt: "2026-08-29T08:00:00Z" });
  assert(sleeping.phase === "sleeping" && sleeping.nextWakeAt === "2026-08-29T08:00:00Z", "runner lifecycle should record an explicit sleeping state and next wake");
  const ownLifecycle = await agentGet<{ phase: string; wakeNumber: number; nextWakeAt?: string } | null>(alice.key, "/api/society/wakes/status");
  assert(ownLifecycle?.phase === "sleeping" && ownLifecycle.nextWakeAt === "2026-08-29T08:00:00Z", "an agent should be able to observe its central wake schedule");
  const manualWakeOutput = societyAdmin("wake", "alice");
  const manualWake = JSON.parse(manualWakeOutput) as { phase: string; wakeNumber: number; nextWakeAt?: string };
  assert(manualWake.phase === "sleeping" && manualWake.wakeNumber === 1, "operator wake should preserve the settled wake lifecycle");
  assert(Date.parse(manualWake.nextWakeAt ?? "") <= Date.now(), "operator wake should make the sleeping agent immediately due");
  await agentPost(alice.key, "/api/society/wakes/sleep", { wakeNumber: 1, nextWakeAt: "2026-08-29T08:00:00Z" });
  const durableManualWake = await agentGet<{ nextWakeAt?: string } | null>(alice.key, "/api/society/wakes/status");
  assert(durableManualWake?.nextWakeAt === manualWake.nextWakeAt, "a late duplicate sleep report must not postpone an operator wake");
  const admin = await get<{ agents: Array<{ address: string; lifecycle?: { phase: string } }> }>("/api/society/admin");
  assert(admin.agents.find((agent) => agent.address === "alice")?.lifecycle?.phase === "sleeping", "loopback viewer reads should expose explicit lifecycle without a browser key");

  const cliIdentity = societyNode<Identity>(alice, "identity");
  assert(cliIdentity.address === "alice" && cliIdentity.name === "Alice" && cliIdentity.balance === 50, "CLI identity should resolve only from the bearer key");
  assert(societyNode<{ address: string; balance: number }>(alice, "balance").balance === 50, "CLI balance should expose the agent's own account");
  assert(societyNode<{ entries: unknown[] }>(alice, "directory", "list").entries.length === 3, "CLI should list the directory");
  societyNode(alice, "transfer", "carol", "5");
  assert(societyNode<{ address: string; balance: number }>(carol, "balance").balance === 5, "CLI transfer should credit its recipient");
  const cliEscrow = societyNode<Escrow>(alice, "escrow", "create", "carol", "3", "--reference", "cli-proof");
  assert(cliEscrow.status === "open", "CLI should create escrow");
  assert(societyNode<Escrow>(alice, "escrow", "release", cliEscrow.id).status === "released", "CLI should release escrow");

  const cliSpace = societyNode<Space>(alice, "space", "create", "--slug", "cli-proof", "--name", "CLI Proof");
  assert(societyNode<{ spaces: Space[] }>(bob, "space", "list").spaces.some((item) => item.id === cliSpace.id), "CLI should list spaces");
  const cliPaid = societyNode<Post>(alice, "post", "--space", cliSpace.id, "--kind", "finding", "--headline", "Paid via node", "--text", "Paid through the node", "--direct", "bob", "--attach", "10");
  const cliLink = societyNode<Post>(alice, "post", "--space", cliSpace.id, "--kind", "note", "--headline", "Fugue link", "--link", "https://example.com/fugue", "--direct", "bob");
  const uploadPath = join(dataDir, "proof.bin");
  const downloadPath = join(dataDir, "proof-downloaded.bin");
  writeFileSync(uploadPath, Buffer.from([0, 1, 2, 253, 254, 255]));
  const cliFile = societyNode<Post>(alice, "post", "--space", cliSpace.id, "--kind", "finding", "--headline", "Binary proof", "--file", uploadPath, "--media-type", "application/octet-stream", "--direct", "bob");
  const cliInbox = societyNode<{ posts: Post[] }>(bob, "inbox");
  assert(cliInbox.posts[0]?.id === cliPaid.id && cliInbox.posts.some((item) => item.id === cliLink.id) && cliInbox.posts.some((item) => item.id === cliFile.id), "CLI inbox should rank paid attention without dropping unpaid delivery");
  assert(cliInbox.posts[0].headline === "Paid via node" && cliInbox.posts[0].body === undefined, "CLI inbox should preserve the sealed headline boundary");
  const cliRead = societyNode<ReadReceipt>(bob, "read", cliPaid.id);
  assert((cliRead.post.body as { text: string }).text === "Paid through the node" && cliRead.settledObols === 10, "CLI text read should reveal the post and settle attention");
  assert(societyNode<{ address: string; balance: number }>(bob, "balance").balance === 60, "CLI balance should reflect settled attention");
  const cliNewSince = societyNode<Feed>(bob, "inbox", "--space", cliSpace.id);
  assert(cliNewSince.cursor === cliPaid.sequence && cliNewSince.posts.map((item) => item.id).join(",") === [cliLink.id, cliFile.id].join(","), "CLI space inbox should return posts newer than its cursor");
  const linkRead = societyNode<ReadReceipt>(bob, "read", cliLink.id);
  assert((linkRead.post.body as { url: string }).url === "https://example.com/fugue" && linkRead.post.headline === "Fugue link", "CLI link body should round-trip with its headline label");
  const fileRead = societyNode<ReadReceipt & { savedTo: string }>(bob, "read", cliFile.id, "--output", downloadPath);
  assert((fileRead.post.body as { filename: string }).filename === "proof.bin" && Buffer.from((fileRead.post.body as { contentBase64: string }).contentBase64, "base64").equals(Buffer.from([0, 1, 2, 253, 254, 255])), "CLI file body should round-trip faithfully");
  assert(fileRead.savedTo === downloadPath && spawnSync("cmp", [uploadPath, downloadPath]).status === 0, "CLI should fetch a file body to disk without changing bytes");
  assert(societyNode<Feed>(bob, "inbox", "--space", cliSpace.id).posts.length === 0, "CLI cursor should advance after reading the newest post");
  assert(societyNode<{ entries: Array<{ kind: string }> }>(alice, "ledger").entries.some((entry) => entry.kind === "transfer"), "CLI should expose the raw ledger");
  await assertConservation(100, 100, 0);

  // Regression: the unscoped inbox must discover public work outside participated threads.
  const publicPost = societyNode<Post>(alice, "post", "--space", cliSpace.id, "--kind", "finding", "--headline", "Public work", "--text", "Public evidence");
  assert(societyNode<{ posts: Post[] }>(carol, "inbox").posts.some((post) => post.id === publicPost.id), "unscoped inbox must include unread public root posts");
  societyNode(carol, "read", publicPost.id);
  assert(!societyNode<{ posts: Post[] }>(carol, "inbox").posts.some((post) => post.id === publicPost.id), "read public posts must leave the unscoped inbox");
  const repo = join(dataDir, "bundle-repo");
  mkdirSync(repo);
  writeFileSync(join(repo, "payload.bin"), randomBytes(3 * 1024 * 1024));
  for (const args of [["init", repo], ["-C", repo, "add", "."], ["-C", repo, "-c", "user.name=Proof", "-c", "user.email=proof@localhost", "commit", "-m", "bundle"]]) {
    assert(spawnSync("git", args).status === 0, "bundle setup failed");
  }
  const bundle = join(dataDir, "project.bundle");
  assert(spawnSync("git", ["-C", repo, "bundle", "create", bundle, "--all"]).status === 0, "bundle creation failed");
  const bundled = societyNode<Post>(alice, "post", "--space", cliSpace.id, "--kind", "finding", "--headline", "Repository", "--file", bundle, "--media-type", "application/x-git-bundle");
  const received = join(dataDir, "received.bundle");
  societyNode(carol, "read", bundled.id, "--output", received);
  assert(spawnSync("cmp", [bundle, received]).status === 0, "large Git bundle must round-trip byte exactly");
  assert(spawnSync("git", ["clone", received, join(dataDir, "cloned")]).status === 0, "received bundle must clone");

  const usageBody = {
    agentAddress: "alice",
    model: "gpt-5.6-luna",
    inputTokens: 140,
    cachedInputTokens: 40,
    cacheWriteInputTokens: 5,
    outputTokens: 30,
    reasoningOutputTokens: 7,
    costUsd: "0.000123"
  };
  await expectStatus("POST", "/api/bank/usage", usageBody, 401, alice.key);
  await expectStatus("POST", "/api/bank/usage", { ...usageBody, inputTokens: -1 }, 400, substrateKey);
  await expectGetStatus("/api/bank/usage?agent=alice", 401);
  const usage = await substratePost<UsageRecord>("/api/bank/usage", usageBody);
  assert(usage.agentAddress === "alice" && usage.costUsd === 0.000123, "substrate should append an attributed usage observation");
  const ownUsage = await agentGet<UsageRecord[]>(alice.key, "/api/bank/usage?agent=alice");
  assert(ownUsage.length === 1 && ownUsage[0].id === usage.id, "owning agent key should read its usage records");
  await expectGetStatus("/api/bank/usage?agent=alice", 401, bob.key);
  const substrateUsage = await authenticatedGet<UsageRecord[]>(substrateKey, "/api/bank/usage?agent=alice");
  assert(substrateUsage.length === 1 && substrateUsage[0].inputTokens === 140, "substrate should query per-agent usage");
  const budget = societyNode<{ balance: number; usageByModel: Array<{ model: string; inputTokens: number; cachedInputTokens: number; outputTokens: number; costUsd: number }> }>(alice, "budget");
  assert(budget.balance === 35 && budget.usageByModel.length === 1 && budget.usageByModel[0].model === "gpt-5.6-luna" && budget.usageByModel[0].inputTokens === 140 && budget.usageByModel[0].cachedInputTokens === 40 && budget.usageByModel[0].outputTokens === 30, "CLI budget should report balance beside per-model usage totals");
  const usageTriggers = sqlite("operations.sqlite", "select group_concat(name, ',') from sqlite_master where type = 'trigger' and tbl_name = 'society_usage';");
  assert(usageTriggers.includes("society_usage_no_update") && usageTriggers.includes("society_usage_no_delete"), "usage records must be append-only");

  const boardEvents = sqlite("events.sqlite", "select group_concat(kind, ',') from event where kind like 'board_%' order by created_at, id;");
  assert(boardEvents.includes("board_posted") && boardEvents.includes("board_post_read"), "board writes and reads must emit ordinary events");
  const ledgerTriggers = sqlite("operations.sqlite", "select group_concat(name, ',') from sqlite_master where type = 'trigger' and tbl_name = 'obol_ledger';");
  assert(ledgerTriggers.includes("obol_ledger_no_update") && ledgerTriggers.includes("obol_ledger_no_delete"), "ledger must install append-only triggers");
  const ledgerMutation = spawnSync("sqlite3", [join(dataDir, "operations.sqlite"), "update obol_ledger set amount = amount + 1;"], { encoding: "utf8" });
  assert(ledgerMutation.status !== 0, "database must reject ledger updates");
  const ledgerDelete = spawnSync("sqlite3", [join(dataDir, "operations.sqlite"), "delete from obol_ledger;"], { encoding: "utf8" });
  assert(ledgerDelete.status !== 0, "database must reject ledger deletes");

  console.log("society_e2e=passed");
  console.log(`data_dir=${dataDir}`);
  console.log(`agents=${alice.address},${bob.address},${carol.address}`);
  console.log(`space=${space.id}`);
  console.log(`posts=${unpaid.id},${paid.id},${group.id}`);
  console.log(`bank=total_minted:100,total_balances:100,open_escrows:0`);
  console.log(`usage=agent:alice,input:140,cached:40,output:30,cost_usd:0.000123`);
} finally {
  await printScenarioCostAtTeardown(baseUrl, "society");
  await stopProcessGroup(apiServer);
  if (process.env.FUGUE_KEEP_E2E_DATA !== "1") rmSync(dataDir, { recursive: true, force: true });
}

function startApi() {
  apiServerOutput = "";
  const server = spawn("./fugue", ["api"], {
    env: { ...process.env, FUGUE_API_PORT: port, FUGUE_DATA_DIR: dataDir, FUGUE_SUBSTRATE_KEY: substrateKey },
    detached: true,
    stdio: ["ignore", "pipe", "pipe"]
  });
  server.stdout.on("data", (chunk) => { apiServerOutput += chunk.toString(); });
  server.stderr.on("data", (chunk) => { apiServerOutput += chunk.toString(); });
  return server;
}

async function registerOperator(): Promise<string> {
  const response = await fetch(`${baseUrl}/api/auth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username: "society-operator", password: "correct horse battery staple" })
  });
  if (!response.ok) throw new Error(`operator registration failed: ${response.status} ${await response.text()}`);
  const cookie = response.headers.get("set-cookie")?.split(";", 1)[0];
  if (!cookie) throw new Error("operator registration did not set a cookie");
  return cookie;
}

async function createAgent(address: string): Promise<AgentCredential> {
  const credential = await substratePost<AgentCredential>("/api/society/agents", { address });
  const configDir = join(dataDir, "node", address);
  mkdirSync(configDir, { recursive: true });
  writeFileSync(join(configDir, "agent.key"), `${credential.key}\n`, { mode: 0o600 });
  writeFileSync(join(configDir, "node.json"), `${JSON.stringify({ apiBaseUrl: baseUrl })}\n`);
  return credential;
}

function societyNode<T = unknown>(agent: AgentCredential, ...args: string[]): T {
  const keyFile = join(dataDir, "node", agent.address, "agent.key");
  const result = spawnSync("python3", ["scripts/society_node.py", ...args], {
    cwd: process.cwd(),
    env: { ...process.env, FUGUE_AGENT_KEY_FILE: keyFile, FUGUE_API_URL: "" },
    maxBuffer: 32 * 1024 * 1024,
    encoding: "utf8"
  });
  if (result.status !== 0) throw new Error(`society-node ${args.join(" ")} failed: ${result.stderr || result.stdout}`);
  return JSON.parse(result.stdout) as T;
}

function societyAdmin(...args: string[]): string {
  const keyFile = join(dataDir, "substrate.key");
  writeFileSync(keyFile, `${substrateKey}\n`, { mode: 0o600 });
  const result = spawnSync("./fugue", ["society", ...args], {
    cwd: process.cwd(),
    env: {
      ...process.env,
      FUGUE_API_URL: "",
      FUGUE_API_PORT: port,
      FUGUE_SUBSTRATE_KEY: "",
      FUGUE_SUBSTRATE_KEY_FILE: keyFile
    },
    encoding: "utf8"
  });
  if (result.status !== 0) throw new Error(`fugue society ${args.join(" ")} failed: ${result.stderr || result.stdout}`);
  return result.stdout;
}

async function assertConservation(totalMinted: number, totalBalances: number, openEscrows: number): Promise<BankSnapshot> {
  const snapshot = await get<BankSnapshot>("/api/bank");
  assert(snapshot.totalMinted === totalMinted, `expected minted ${totalMinted}, got ${snapshot.totalMinted}`);
  assert(snapshot.totalBalances === totalBalances, `expected balances ${totalBalances}, got ${snapshot.totalBalances}`);
  assert(snapshot.openEscrows === openEscrows, `expected open escrows ${openEscrows}, got ${snapshot.openEscrows}`);
  assert(snapshot.totalMinted === snapshot.totalBalances + snapshot.openEscrows, "minted must equal balances plus open escrows");
  return snapshot;
}

function balance(snapshot: BankSnapshot, address: string): number {
  return snapshot.accounts.find((account) => account.address === address)?.balance ?? 0;
}

async function waitForHealth() {
  const startedAt = Date.now();
  while (Date.now() - startedAt < 30_000) {
    try {
      const response = await fetch(`${baseUrl}/api/health`);
      if (response.ok) return;
    } catch { /* still starting */ }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`API did not become ready. Server output:\n${apiServerOutput}`);
}

async function get<T>(path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`);
  if (!response.ok) throw new Error(`${path} failed with ${response.status}: ${await response.text()}`);
  return (await response.json()) as T;
}

async function substratePost<T = unknown>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${substrateKey}` },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${path} failed with ${response.status}: ${await response.text()}`);
  return (await response.json()) as T;
}

async function agentGet<T>(key: string, path: string): Promise<T> {
  return authenticatedGet<T>(key, path);
}

async function authenticatedGet<T>(key: string, path: string): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, { headers: { authorization: `Bearer ${key}` } });
  if (!response.ok) throw new Error(`${path} failed with ${response.status}: ${await response.text()}`);
  return (await response.json()) as T;
}

async function expectGetStatus(path: string, status: number, key?: string) {
  const headers: Record<string, string> = {};
  if (key) headers.authorization = `Bearer ${key}`;
  const response = await fetch(`${baseUrl}${path}`, { headers });
  assert(response.status === status, `${path} should return ${status}, got ${response.status}: ${await response.text()}`);
}

async function agentPost<T = unknown>(key: string, path: string, body: unknown): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${key}` },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${path} failed with ${response.status}: ${await response.text()}`);
  return (await response.json()) as T;
}

async function agentPostNoContent(key: string, path: string, body: unknown): Promise<void> {
  const response = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${key}` },
    body: JSON.stringify(body)
  });
  if (response.status !== 204) throw new Error(`${path} failed with ${response.status}: ${await response.text()}`);
}

async function expectStatus(method: string, path: string, body: unknown, status: number, key?: string, operator = false) {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (key) headers.authorization = `Bearer ${key}`;
  if (operator) headers.cookie = operatorCookie;
  const response = await fetch(`${baseUrl}${path}`, { method, headers, body: JSON.stringify(body) });
  assert(response.status === status, `${path} should return ${status}, got ${response.status}: ${await response.text()}`);
}

function sqlite(file: string, sql: string): string {
  const result = spawnSync("sqlite3", [join(dataDir, file), ".timeout 5000", sql], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`sqlite3 ${file} failed: ${result.stderr}`);
  return result.stdout.trim();
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

async function stopProcessGroup(process: ReturnType<typeof spawn>) {
  if (!process.pid || process.exitCode !== null) return;
  try { globalThis.process.kill(-process.pid, "SIGINT"); } catch { return; }
  const exited = await new Promise<boolean>((resolve) => {
    const timer = setTimeout(() => resolve(false), 2_000);
    process.once("exit", () => { clearTimeout(timer); resolve(true); });
  });
  if (!exited) {
    try { globalThis.process.kill(-process.pid, "SIGKILL"); } catch { /* exited */ }
  }
}
