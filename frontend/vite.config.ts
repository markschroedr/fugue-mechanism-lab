import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { sveltekit } from "@sveltejs/kit/vite";
import { parse } from "smol-toml";
import { defineConfig } from "vite";

type FugueToml = {
  storage: {
    data_dir: string;
  };
  api: {
    host: string;
    port: number;
  };
  ui: {
    host: string;
    port: number;
    preview_port: number;
  };
  llm: {
    model: string;
    effort: string;
    request_timeout_seconds: number;
    agents?: Record<string, { model?: string; effort?: string }>;
  };
  sources: {
    forum_magnum: {
      endpoint: string;
    };
  };
  society?: Record<string, unknown>;
};

const defaultConfigPath = fileURLToPath(new URL("../fugue.toml", import.meta.url));
const configPath = process.env.FUGUE_CONFIG ?? defaultConfigPath;
const config = readConfig(configPath);

const apiHost = process.env.FUGUE_API_HOST ?? requiredHost("api.host", config.api.host);
const apiPort = environmentPort("FUGUE_API_PORT") ?? requiredPort("api.port", config.api.port);
const uiHost = process.env.FUGUE_UI_HOST ?? requiredHost("ui.host", config.ui.host);
const uiPort = environmentPort("FUGUE_UI_PORT") ?? requiredPort("ui.port", config.ui.port);
const previewPort =
  environmentPort("FUGUE_PREVIEW_PORT") ?? requiredPort("ui.preview_port", config.ui.preview_port);
const labPort = environmentPort("FUGUE_LAB_PORT") ?? 49320;
const apiUrl =
  process.env.VITE_FUGUE_API_URL ??
  `http://${apiHost.includes(":") ? `[${apiHost}]` : apiHost}:${apiPort}`;

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    host: uiHost,
    port: uiPort,
    strictPort: true,
    proxy: {
      "/api/lab": {
        target: `http://127.0.0.1:${labPort}`,
        configure(proxy) {
          proxy.on("error", (_error, _request, response) => {
            if (!("writeHead" in response)) return;
            if (response.headersSent) return;
            response.writeHead(404, { "content-type": "application/json" });
            response.end(JSON.stringify({ error: "Lab observer is not available" }));
          });
        }
      },
      "/api": apiUrl
    }
  },
  preview: {
    host: uiHost,
    port: previewPort,
    strictPort: true
  }
});

function readConfig(path: string): FugueToml {
  const root = requiredTable("config", parse(readFileSync(path, "utf8")));
  assertExactKeys("config", root, ["storage", "api", "ui", "llm", "sources", "society"]);

  const storage = requiredTable("storage", root.storage);
  const api = requiredTable("api", root.api);
  const ui = requiredTable("ui", root.ui);
  const llm = requiredTable("llm", root.llm);
  const sources = requiredTable("sources", root.sources);
  const forumMagnum = requiredTable("sources.forum_magnum", sources.forum_magnum);

  assertExactKeys("storage", storage, ["data_dir"]);
  assertExactKeys("api", api, ["host", "port"]);
  assertExactKeys("ui", ui, ["host", "port", "preview_port"]);
  assertExactKeys("llm", llm, ["model", "effort", "request_timeout_seconds", "agents"]);
  assertExactKeys("sources", sources, ["forum_magnum"]);
  assertExactKeys("sources.forum_magnum", forumMagnum, ["endpoint"]);

  if (root.society !== undefined) {
    const society = requiredTable("society", root.society);
    assertExactKeys("society", society, [
      "world", "instrumentation", "instance_id", "mission", "treasury_address", "weekly_allowance", "compute_backing", "shared_daily_line",
      "guest_api_url", "kit_dir", "sandbox_bin", "runner", "models"
    ]);
    const runner = requiredTable("society.runner", society.runner);
    assertExactKeys("society.runner", runner, [
      "executor_binary", "default_model", "default_effort",
      "default_sleep_seconds", "max_sleep_seconds", "active_hours_timezone", "active_hours_start",
      "active_hours_end", "wall_timeout_seconds", "poll_seconds",
      "quota_threshold_percent", "quota_pause_seconds", "quota_probe",
      "wake_gate_path", "max_active_wakes", "wake_lease_seconds"
    ]);
    const models = requiredTable("society.models", society.models);
    assertExactKeys("society.models", models, ["luna", "terra", "sol"]);
    for (const alias of ["luna", "terra", "sol"]) {
      assertExactKeys(`society.models.${alias}`, requiredTable(`society.models.${alias}`, models[alias]), [
        "model", "inputUsdPerMillion", "cachedInputUsdPerMillion",
        "cacheWriteInputUsdPerMillion", "outputUsdPerMillion"
      ]);
    }
  }

  if (llm.agents !== undefined) {
    const agents = requiredTable("llm.agents", llm.agents);
    for (const [name, profile] of Object.entries(agents)) {
      assertExactKeys(`llm.agents.${name}`, requiredTable(`llm.agents.${name}`, profile), [
        "model",
        "effort"
      ]);
    }
  }

  return root as FugueToml;
}

function requiredTable(name: string, value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${name} must be a TOML table in ${configPath}`);
  }
  return value as Record<string, unknown>;
}

function assertExactKeys(name: string, value: Record<string, unknown>, expected: string[]) {
  const unknown = Object.keys(value).filter((key) => !expected.includes(key));
  if (unknown.length > 0) {
    throw new Error(`${name} contains unknown field(s) ${unknown.join(", ")} in ${configPath}`);
  }
}

function requiredHost(name: string, value: unknown): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${name} must be a non-empty string in ${configPath}`);
  }
  return value;
}

function requiredPort(name: string, value: unknown): number {
  if (!Number.isInteger(value) || Number(value) < 1 || Number(value) > 65_535) {
    throw new Error(`${name} must be a valid non-zero TCP port in ${configPath}`);
  }
  return Number(value);
}

function environmentPort(name: string): number | undefined {
  const value = process.env[name];
  if (value === undefined) return undefined;
  return requiredPort(name, Number(value));
}
