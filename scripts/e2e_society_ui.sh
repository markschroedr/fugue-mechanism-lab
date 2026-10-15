#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
API_PORT="${FUGUE_SOCIETY_UI_E2E_API_PORT:-49631}"
UI_PORT="${FUGUE_SOCIETY_UI_E2E_UI_PORT:-49632}"
TEMP_BASE="${TMPDIR:-/tmp}"
TEST_ROOT="$(mktemp -d "$TEMP_BASE/fugue-society-ui-e2e.XXXXXX")"
BROWSER_SESSION="fugue-society-ui-e2e-$$"
SUBSTRATE_KEY="$(bun -e 'console.log(Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("hex"))')"
API_PID=""
UI_PID=""

cleanup() {
  agent-browser --session "$BROWSER_SESSION" close >/dev/null 2>&1 || true
  if [[ -n "$UI_PID" ]]; then kill "$UI_PID" >/dev/null 2>&1 || true; wait "$UI_PID" >/dev/null 2>&1 || true; fi
  if [[ -n "$API_PID" ]]; then kill "$API_PID" >/dev/null 2>&1 || true; wait "$API_PID" >/dev/null 2>&1 || true; fi
  case "$TEST_ROOT" in "$TEMP_BASE"/fugue-society-ui-e2e.*) rm -rf -- "$TEST_ROOT" ;; esac
}
trap cleanup EXIT INT TERM

wait_for_url() {
  local url="$1" log="$2"
  for _ in $(seq 1 480); do
    if curl --fail --silent --show-error "$url" >/dev/null 2>&1; then return; fi
    sleep 0.25
  done
  printf 'Timed out waiting for %s\n' "$url" >&2
  sed -n '1,220p' "$log" >&2
  exit 1
}

command -v agent-browser >/dev/null || { printf 'agent-browser is required\n' >&2; exit 1; }
cd "$ROOT"
env -u OPENAI_API_KEY -u OPENROUTER_API_KEY \
  FUGUE_EXPECT_ZERO_LLM=1 FUGUE_DATA_DIR="$TEST_ROOT/data" FUGUE_API_PORT="$API_PORT" \
  FUGUE_SUBSTRATE_KEY="$SUBSTRATE_KEY" ./fugue api >"$TEST_ROOT/api.log" 2>&1 &
API_PID="$!"
env -u OPENAI_API_KEY -u OPENROUTER_API_KEY \
  FUGUE_API_PORT="$API_PORT" FUGUE_UI_PORT="$UI_PORT" ./fugue ui >"$TEST_ROOT/ui.log" 2>&1 &
UI_PID="$!"
wait_for_url "http://127.0.0.1:$API_PORT/api/health" "$TEST_ROOT/api.log"
wait_for_url "http://127.0.0.1:$UI_PORT/society" "$TEST_ROOT/ui.log"
FUGUE_UI_E2E_API_URL="http://127.0.0.1:$API_PORT" FUGUE_SUBSTRATE_KEY="$SUBSTRATE_KEY" \
  bun run scripts/seed_society_ui.ts >"$TEST_ROOT/seed.log"

agent-browser --session "$BROWSER_SESSION" set viewport 1280 800 >/dev/null
agent-browser --session "$BROWSER_SESSION" open "http://127.0.0.1:$UI_PORT/society" >/dev/null
agent-browser --session "$BROWSER_SESSION" wait '.society-shell' >/dev/null
agent-browser --session "$BROWSER_SESSION" eval '(async () => {
  const rows = [...document.querySelectorAll(".agent-row")];
  const text = (document.body.textContent || "").replace(/\s+/g, " ");
  if (rows.length !== 2) throw new Error(`expected two agent rows, got ${rows.length}`);
  const expectedAvatars = new Map([
    ["robinson", "/assets/creatures/creature-03-asleep.png"],
    ["friday", "/assets/creatures/creature-07-awake.png"]
  ]);
  for (const row of rows) {
    const address = row.getAttribute("data-agent");
    const avatar = row.querySelector("img.agent-avatar");
    if (!(avatar instanceof HTMLImageElement)) throw new Error(`missing avatar for ${address}`);
    await avatar.decode();
    if (avatar.naturalWidth !== 256 || avatar.naturalHeight !== 256) throw new Error(`invalid avatar size for ${address}`);
    if (!avatar.src.endsWith(expectedAvatars.get(address) || "missing")) throw new Error(`unstable avatar assignment for ${address}`);
    const canvas = document.createElement("canvas");
    canvas.width = avatar.naturalWidth;
    canvas.height = avatar.naturalHeight;
    const context = canvas.getContext("2d");
    context.drawImage(avatar, 0, 0);
    if (context.getImageData(0, 0, 1, 1).data[3] !== 0) throw new Error(`avatar corner is not transparent for ${address}`);
  }
  for (const value of ["robinson", "friday", "88 Obols", "104 Obols", "First light", "Sealed group bounty", "Withheld", "7 Obols", "Escrow returned robinson 3 Obols", "Escrow released to friday 4 Obols"]) {
    if (!text.includes(value)) throw new Error(`missing ${value}`);
  }
  if (text.includes("UI-SEALED-BODY-MUST-NOT-LEAK")) throw new Error("sealed body leaked into the UI");
  if (!text.includes("Sleeping · Wake 1") || !text.includes("Awake · Wake 1")) throw new Error("explicit lifecycle state is missing");
  if (document.querySelector("input[aria-label=\"Substrate key\"]") || text.includes(" Lock")) throw new Error("viewer still exposes a substrate-key gate");
  return { agents: rows.length, sealed: "withheld" };
})()' >/dev/null
agent-browser --session "$BROWSER_SESSION" click 'button[data-agent="robinson"]' >/dev/null
agent-browser --session "$BROWSER_SESSION" wait '.agent-overview' >/dev/null
agent-browser --session "$BROWSER_SESSION" eval '(() => {
  const overview = document.querySelector(".agent-overview");
  const text = overview?.textContent || "";
  const avatar = overview?.querySelector("img.agent-avatar");
  if (!(avatar instanceof HTMLImageElement) || !avatar.src.endsWith("/assets/creatures/creature-03-asleep.png")) throw new Error("agent overview avatar does not match explicit sleeping state");
  for (const value of ["Wake 1", "gpt-5.6-luna", "$0.000190", "completed", "First light", "These three documents are your spawn document", "bash", "society-node inbox", "First wake complete."]) {
    if (!text.includes(value)) throw new Error(`agent overview missing ${value}`);
  }
  return { receipts: 1, transcript: "readable" };
})()' >/dev/null
ADMIN_JSON="$(curl -fsS "http://127.0.0.1:$API_PORT/api/society/admin")"
DETAIL_JSON="$(curl -fsS "http://127.0.0.1:$API_PORT/api/society/admin/agents/robinson")"
if grep -q 'UI-SEALED-BODY-MUST-NOT-LEAK' <<<"$ADMIN_JSON$DETAIL_JSON"; then
  printf 'Admin transport leaked a sealed body\n' >&2
  exit 1
fi

printf 'society_ui_e2e=passed\n'
printf 'viewer_access=loopback_without_browser_key\n'
printf 'sidebar=agents:2,balances:true,lifecycle:explicit,avatars:awake_and_sleeping_rgba\n'
printf 'commons=chronological,sealed_body_withheld:true\n'
printf 'agent_overview=receipts,posts,readable_transcript\n'
FUGUE_EXPECT_ZERO_LLM=1 FUGUE_REQUIRE_SCENARIO_COST=1 bun -e \
  'import { printScenarioCostAtTeardown } from "./scripts/lib/scenario_cost"; await printScenarioCostAtTeardown(process.argv[1], "society-browser-ui");' \
  "http://127.0.0.1:$API_PORT"
