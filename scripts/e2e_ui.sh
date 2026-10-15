#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
API_PORT="${FUGUE_UI_E2E_API_PORT:-49621}"
UI_PORT="${FUGUE_UI_E2E_UI_PORT:-49622}"
TEMP_BASE="${TMPDIR:-/tmp}"
TEST_ROOT="$(mktemp -d "$TEMP_BASE/fugue-ui-e2e.XXXXXX")"
BROWSER_SESSION="fugue-ui-e2e-$$"
API_PID=""
UI_PID=""

cleanup() {
  agent-browser --session "$BROWSER_SESSION" close >/dev/null 2>&1 || true
  if [[ -n "$UI_PID" ]]; then
    kill "$UI_PID" >/dev/null 2>&1 || true
    wait "$UI_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$API_PID" ]]; then
    kill "$API_PID" >/dev/null 2>&1 || true
    wait "$API_PID" >/dev/null 2>&1 || true
  fi
  case "$TEST_ROOT" in
    "$TEMP_BASE"/fugue-ui-e2e.*) rm -rf -- "$TEST_ROOT" ;;
  esac
}
trap cleanup EXIT INT TERM

wait_for_url() {
  local url="$1"
  local log="$2"
  for _ in $(seq 1 480); do
    if curl --fail --silent --show-error "$url" >/dev/null 2>&1; then
      return
    fi
    sleep 0.25
  done
  printf 'Timed out waiting for %s\n' "$url" >&2
  sed -n '1,220p' "$log" >&2
  exit 1
}

command -v agent-browser >/dev/null || {
  printf 'agent-browser is required for ./fugue e2e-ui\n' >&2
  exit 1
}

cd "$ROOT"
env -u OPENAI_API_KEY -u OPENROUTER_API_KEY \
  FUGUE_EXPECT_ZERO_LLM=1 FUGUE_DATA_DIR="$TEST_ROOT/data" FUGUE_API_PORT="$API_PORT" \
  ./fugue api >"$TEST_ROOT/api.log" 2>&1 &
API_PID="$!"
env -u OPENAI_API_KEY -u OPENROUTER_API_KEY \
  FUGUE_API_PORT="$API_PORT" FUGUE_UI_PORT="$UI_PORT" \
  ./fugue ui >"$TEST_ROOT/ui.log" 2>&1 &
UI_PID="$!"

wait_for_url "http://127.0.0.1:$API_PORT/api/health" "$TEST_ROOT/api.log"
wait_for_url "http://127.0.0.1:$UI_PORT" "$TEST_ROOT/ui.log"

agent-browser --session "$BROWSER_SESSION" set viewport 390 844 >/dev/null
agent-browser --session "$BROWSER_SESSION" open "http://127.0.0.1:$UI_PORT" >/dev/null
agent-browser --session "$BROWSER_SESSION" wait '.switch' >/dev/null
agent-browser --session "$BROWSER_SESSION" click '.switch' >/dev/null
agent-browser --session "$BROWSER_SESSION" fill 'input[aria-label="Username"]' ui-e2e >/dev/null
agent-browser --session "$BROWSER_SESSION" fill 'input[aria-label="Password"]' fugue-ui-e2e-password >/dev/null
agent-browser --session "$BROWSER_SESSION" click 'button[type="submit"]' >/dev/null
agent-browser --session "$BROWSER_SESSION" wait '.mobile-tabs [role="tab"]:nth-child(3)' >/dev/null
agent-browser --session "$BROWSER_SESSION" eval '(() => {
  const tabs = [...document.querySelectorAll("[role=tab]")];
  const visiblePanels = [...document.querySelectorAll("[role=tabpanel]")].filter((panel) => !panel.hidden);
  const iconButtons = [...document.querySelectorAll("button")].filter((button) => button.querySelector("svg"));
  const unnamedIconButtons = iconButtons.filter(
    (button) => !(button.getAttribute("aria-label") || button.textContent?.trim() || button.getAttribute("title"))
  );
  const nonLucideIcons = iconButtons
    .flatMap((button) => [...button.querySelectorAll("svg")])
    .filter((icon) => !icon.classList.contains("lucide"));
  if (tabs.map((tab) => tab.textContent?.trim()).join(",") !== "Document,Session,Review") {
    throw new Error("mobile pane tabs are missing or out of order");
  }
  if (tabs[0].getAttribute("aria-selected") !== "true") throw new Error("Document tab is not selected");
  if (!tabs[1].hasAttribute("disabled")) throw new Error("inactive Session tab is not disabled");
  if (visiblePanels.length !== 1 || visiblePanels[0].getAttribute("aria-labelledby") !== tabs[0].id) {
    throw new Error("Document tab does not own exactly one visible panel");
  }
  if (unnamedIconButtons.length) throw new Error("an icon button has no accessible name");
  if (nonLucideIcons.length) throw new Error("a button still contains a non-Lucide icon");
  return { tabs: tabs.length, namedIconButtons: iconButtons.length, visiblePanel: "Document" };
})()' >/dev/null

agent-browser --session "$BROWSER_SESSION" focus '[role="tab"][aria-selected="true"]' >/dev/null
agent-browser --session "$BROWSER_SESSION" press ArrowRight >/dev/null
agent-browser --session "$BROWSER_SESSION" wait 100 >/dev/null
agent-browser --session "$BROWSER_SESSION" eval '(() => {
  const selected = document.querySelector("[role=tab][aria-selected=true]");
  const visiblePanel = [...document.querySelectorAll("[role=tabpanel]")].find((panel) => !panel.hidden);
  if (selected?.textContent?.trim() !== "Review") throw new Error("ArrowRight did not skip the disabled Session tab");
  if (!visiblePanel?.textContent?.includes("Nothing waiting on you")) throw new Error("Review panel did not activate");
  return { keyboardSelected: "Review" };
})()' >/dev/null

agent-browser --session "$BROWSER_SESSION" set viewport 1280 800 >/dev/null
agent-browser --session "$BROWSER_SESSION" reload >/dev/null
agent-browser --session "$BROWSER_SESSION" wait '.shell' >/dev/null
agent-browser --session "$BROWSER_SESSION" eval '(() => {
  const shell = document.querySelector(".shell");
  const body = document.querySelector(".body");
  if (!shell || getComputedStyle(shell).display !== "flex") throw new Error("Tabs root changed the desktop shell layout");
  if (shell.hasAttribute("role")) throw new Error("Tabs root added an unrelated desktop role");
  if (document.querySelector(".mobile-tabs")) throw new Error("mobile tab list rendered at desktop width");
  if (!body || getComputedStyle(body).gridTemplateColumns.split(" ").length !== 3) {
    throw new Error("desktop three-pane layout changed");
  }
  return { desktopColumns: getComputedStyle(body).gridTemplateColumns };
})()' >/dev/null

if rg -n '<svg|</svg>|>\s*[✓≈+]\s*(\{|<)|^\s*☰\s*$|>×<' frontend/src --glob '*.svelte' >"$TEST_ROOT/hand-authored-icons.log"; then
  printf 'Hand-authored icon markup remains:\n' >&2
  sed -n '1,120p' "$TEST_ROOT/hand-authored-icons.log" >&2
  exit 1
fi

printf 'ui_e2e=passed\n'
printf 'mobile_tabs=document,session,review\n'
printf 'keyboard_navigation=document_to_review_skipping_disabled_session\n'
printf 'desktop_layout=three_panes\n'
printf 'icons=lucide_with_accessible_button_names\n'
FUGUE_EXPECT_ZERO_LLM=1 FUGUE_REQUIRE_SCENARIO_COST=1 bun -e \
  'import { printScenarioCostAtTeardown } from "./scripts/lib/scenario_cost"; await printScenarioCostAtTeardown(process.argv[1], "browser-ui");' \
  "http://127.0.0.1:$API_PORT"
