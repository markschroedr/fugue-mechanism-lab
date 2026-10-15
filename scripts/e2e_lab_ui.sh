#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMP_BASE="${TMPDIR:-/tmp}"
TEST_ROOT="$(mktemp -d "$TEMP_BASE/fugue-lab-ui-e2e.XXXXXX")"
HOST_CONFIG="$TEST_ROOT/host.toml"
LAB_PORT="${FUGUE_LAB_UI_E2E_LAB_PORT:-49641}"
UI_PORT="${FUGUE_LAB_UI_E2E_UI_PORT:-49642}"
BROWSER_SESSION="fugue-lab-ui-e2e-$$"
ALPHA_API_PID=""
BETA_API_PID=""
LAB_PID=""
UI_PID=""

stop_process() {
  local pid="$1"
  if [[ -n "$pid" ]] && kill -0 "$pid" >/dev/null 2>&1; then
    kill "$pid" >/dev/null 2>&1 || true
    wait "$pid" >/dev/null 2>&1 || true
  fi
}

cleanup() {
  agent-browser --session "$BROWSER_SESSION" close >/dev/null 2>&1 || true
  stop_process "$UI_PID"
  stop_process "$LAB_PID"
  stop_process "$BETA_API_PID"
  stop_process "$ALPHA_API_PID"
  case "$TEST_ROOT" in
    "$TEMP_BASE"/fugue-lab-ui-e2e.*) rm -rf -- "$TEST_ROOT" ;;
  esac
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

binding_port() {
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["apiPort"])' "$1"
}

seed_agent() {
  local api_port="$1" key_file="$2" address="$3" name="$4" output="$5" key
  key="$(<"$key_file")"
  curl --fail --silent --show-error \
    -H 'content-type: application/json' \
    -H "authorization: Bearer $key" \
    --data-binary "{\"address\":\"$address\",\"name\":\"$name\",\"key\":\"$key\"}" \
    "http://127.0.0.1:$api_port/api/society/birth" >"$output"
}

command -v agent-browser >/dev/null || { printf 'agent-browser is required\n' >&2; exit 1; }
command -v tar >/dev/null || { printf 'tar is required\n' >&2; exit 1; }

cd "$ROOT"
unset OPENAI_API_KEY OPENROUTER_API_KEY || true
mkdir -p "$TEST_ROOT/experiments" "$TEST_ROOT/secrets" "$TEST_ROOT/logs" "$TEST_ROOT/gateways"
cat >"$HOST_CONFIG" <<EOF
experiments_dir = "$TEST_ROOT/experiments"
secrets_dir = "$TEST_ROOT/secrets"
logs_dir = "$TEST_ROOT/logs"
gateways_dir = "$TEST_ROOT/gateways"
vm_bridge_host = "127.0.0.1"
sandbox_bin = ""
server_bin = "$ROOT/target/debug/fugue-server"
caddy_bin = ""
EOF

./fugue cargo build -q -p fugue-server -p fugue-lab
FUGUE_HOST_CONFIG="$HOST_CONFIG" FUGUE_CONFIG="$ROOT/fugue.toml" ./fugue experiment create alpha >"$TEST_ROOT/create-alpha.log"
FUGUE_HOST_CONFIG="$HOST_CONFIG" FUGUE_CONFIG="$ROOT/fugue.toml" ./fugue experiment create beta >"$TEST_ROOT/create-beta.log"

ALPHA_CONFIG="$TEST_ROOT/experiments/alpha/fugue.toml"
BETA_CONFIG="$TEST_ROOT/experiments/beta/fugue.toml"
ALPHA_PORT="$(binding_port "$TEST_ROOT/gateways/alpha/binding.json")"
BETA_PORT="$(binding_port "$TEST_ROOT/gateways/beta/binding.json")"
ALPHA_KEY_FILE="$TEST_ROOT/secrets/alpha/substrate.key"
BETA_KEY_FILE="$TEST_ROOT/secrets/beta/substrate.key"

FUGUE_CONFIG="$ALPHA_CONFIG" FUGUE_SUBSTRATE_KEY="$(<"$ALPHA_KEY_FILE")" \
  FUGUE_SERVER_BIN="$ROOT/target/debug/fugue-server" ./fugue api >"$TEST_ROOT/alpha-api.log" 2>&1 &
ALPHA_API_PID="$!"
wait_for_url "http://127.0.0.1:$ALPHA_PORT/api/health" "$TEST_ROOT/alpha-api.log"
seed_agent "$ALPHA_PORT" "$ALPHA_KEY_FILE" "alpha-agent" "Alpha Active" "$TEST_ROOT/alpha-seed.json"
rm "$TEST_ROOT/experiments/alpha/.paused"

FUGUE_CONFIG="$BETA_CONFIG" FUGUE_SUBSTRATE_KEY="$(<"$BETA_KEY_FILE")" \
  FUGUE_SERVER_BIN="$ROOT/target/debug/fugue-server" ./fugue api >"$TEST_ROOT/beta-api.log" 2>&1 &
BETA_API_PID="$!"
wait_for_url "http://127.0.0.1:$BETA_PORT/api/health" "$TEST_ROOT/beta-api.log"
seed_agent "$BETA_PORT" "$BETA_KEY_FILE" "robinson" "Beta Robinson" "$TEST_ROOT/beta-seed.json"
FUGUE_HOST_CONFIG="$HOST_CONFIG" FUGUE_CONFIG="$ROOT/fugue.toml" \
  python3 -c 'from scripts.experiment import Host, capture_observer; capture_observer(Host(), "beta")' \
  >"$TEST_ROOT/beta-observer.log"
stop_process "$BETA_API_PID"
BETA_API_PID=""

mkdir -p "$TEST_ROOT/beta-home/notes"
printf 'BETA-PAUSED-HOME-PROOF\nThis file came from the stopped Robinson home snapshot.\n' \
  >"$TEST_ROOT/beta-home/notes/beta.txt"
tar -czf "$TEST_ROOT/experiments/beta/agents/homes/robinson.tar.gz" \
  -C "$TEST_ROOT/beta-home" notes/beta.txt

FUGUE_HOST_CONFIG="$HOST_CONFIG" FUGUE_LAB_PORT="$LAB_PORT" ./fugue lab-api \
  >"$TEST_ROOT/lab.log" 2>&1 &
LAB_PID="$!"
wait_for_url "http://127.0.0.1:$LAB_PORT/api/lab/health" "$TEST_ROOT/lab.log"

FUGUE_CONFIG="$ALPHA_CONFIG" FUGUE_API_PORT="$ALPHA_PORT" FUGUE_UI_PORT="$UI_PORT" \
  FUGUE_LAB_PORT="$LAB_PORT" ./fugue ui >"$TEST_ROOT/ui.log" 2>&1 &
UI_PID="$!"
wait_for_url "http://127.0.0.1:$UI_PORT/society" "$TEST_ROOT/ui.log"

agent-browser --session "$BROWSER_SESSION" set viewport 1280 800 >/dev/null
agent-browser --session "$BROWSER_SESSION" open "http://127.0.0.1:$UI_PORT/society" >/dev/null
agent-browser --session "$BROWSER_SESSION" wait '.society-shell' >/dev/null
agent-browser --session "$BROWSER_SESSION" snapshot -i --compact >"$TEST_ROOT/alpha.snapshot"
agent-browser --session "$BROWSER_SESSION" eval '(() => {
  const selector = document.querySelector("select[aria-label=\"Society experiment\"]");
  if (!(selector instanceof HTMLSelectElement)) throw new Error("missing experiment selector");
  const options = [...selector.options].map((option) => `${option.value}:${option.textContent}`);
  if (options.length !== 2 || !options.includes("alpha:alpha · active") || !options.includes("beta:beta · paused")) throw new Error(`unexpected catalog ${options.join(",")}`);
  const text = document.body.textContent || "";
  if (selector.value !== "alpha" || !text.includes("alpha-agent") || !text.includes("100 Obols")) throw new Error("active alpha data is missing");
  if (text.includes("robinson") || text.includes("BETA-PAUSED-HOME-PROOF")) throw new Error("beta content leaked into alpha");
  return {catalog: options, alpha: "active"};
})()' >/dev/null

agent-browser --session "$BROWSER_SESSION" select 'select[aria-label="Society experiment"]' beta >/dev/null
agent-browser --session "$BROWSER_SESSION" wait 'button[data-agent="robinson"]' >/dev/null
agent-browser --session "$BROWSER_SESSION" snapshot -i --compact >"$TEST_ROOT/beta.snapshot"
agent-browser --session "$BROWSER_SESSION" eval '(() => {
  const text = document.body.textContent || "";
  if (!text.includes("robinson") || !text.includes("100 Obols")) throw new Error("paused beta overview is missing");
  if (text.includes("alpha-agent")) throw new Error("alpha content remained after selecting beta");
  return {beta: "paused"};
})()' >/dev/null
agent-browser --session "$BROWSER_SESSION" click 'button[data-agent="robinson"]' >/dev/null
agent-browser --session "$BROWSER_SESSION" wait '.agent-overview' >/dev/null
agent-browser --session "$BROWSER_SESSION" wait '.file-row[data-file="notes/beta.txt"]' >/dev/null
agent-browser --session "$BROWSER_SESSION" eval '(() => {
  const detail = document.querySelector(".agent-overview")?.textContent || "";
  if (!detail.includes("robinson") || !detail.includes("Beta Robinson")) throw new Error("beta agent detail is missing");
  if (!detail.includes("notes/beta.txt")) throw new Error("paused home file listing is missing");
  return {agent: "robinson", file: "notes/beta.txt"};
})()' >/dev/null
agent-browser --session "$BROWSER_SESSION" click '.file-row[data-file="notes/beta.txt"] button' >/dev/null
agent-browser --session "$BROWSER_SESSION" wait --text 'BETA-PAUSED-HOME-PROOF' >/dev/null

agent-browser --session "$BROWSER_SESSION" select 'select[aria-label="Society experiment"]' alpha >/dev/null
agent-browser --session "$BROWSER_SESSION" wait 'button[data-agent="alpha-agent"]' >/dev/null
agent-browser --session "$BROWSER_SESSION" eval '(() => {
  const text = document.body.textContent || "";
  if (!text.includes("alpha-agent") || !text.includes("100 Obols")) throw new Error("alpha did not reload after beta");
  if (text.includes("robinson") || text.includes("Beta Robinson") || text.includes("notes/beta.txt") || text.includes("BETA-PAUSED-HOME-PROOF")) throw new Error("stale beta content remained after switching to alpha");
  return {alpha: "restored", staleBeta: false};
})()' >/dev/null

printf 'lab_ui_e2e=passed\n'
printf 'catalog=alpha:active,beta:paused\n'
printf 'alpha_view=alpha-agent:100_Obols\n'
printf 'beta_view=robinson:Beta_Robinson,file:notes/beta.txt,preview:true\n'
printf 'stale_isolation=switch_back_clean\n'
FUGUE_EXPECT_ZERO_LLM=1 FUGUE_REQUIRE_SCENARIO_COST=1 bun -e \
  'import { printScenarioCostAtTeardown } from "./scripts/lib/scenario_cost"; await printScenarioCostAtTeardown(process.argv[1], "multi-society-lab-browser-ui");' \
  "http://127.0.0.1:$ALPHA_PORT"
