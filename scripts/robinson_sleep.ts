import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { StringEnum } from "@earendil-works/pi-ai";
import { Type } from "typebox";
import { rename, writeFile } from "node:fs/promises";

const durationPattern = "^(?:\\d+(?:\\.\\d+)?)[smhd]$";
const durationMultipliers = { s: 1, m: 60, h: 3600, d: 86400 } as const;

function durationSeconds(value: string): number {
  const match = /^(\d+(?:\.\d+)?)([smhd])$/.exec(value);
  if (!match) throw new Error("duration must be a number followed by s, m, h, or d");
  return Number(match[1]) * durationMultipliers[match[2] as keyof typeof durationMultipliers];
}

export default function (pi: ExtensionAPI) {
  const models: unknown = JSON.parse(process.env.ROBINSON_MODELS ?? '["luna","terra","sol"]');
  if (!Array.isArray(models) || models.length === 0 || !models.every((model) => typeof model === "string" && model.length > 0)) {
    throw new Error("ROBINSON_MODELS must be a nonempty array of model names");
  }
  const maxSleepSeconds = Number(process.env.ROBINSON_MAX_SLEEP_SECONDS ?? "300");
  if (!Number.isFinite(maxSleepSeconds) || maxSleepSeconds < 0) {
    throw new Error("ROBINSON_MAX_SLEEP_SECONDS must be nonnegative");
  }
  const computeBacking = process.env.ROBINSON_COMPUTE_BACKING ?? "obols";
  if (computeBacking !== "obols" && computeBacking !== "independent") {
    throw new Error("ROBINSON_COMPUTE_BACKING must be obols or independent");
  }
  const capDescription = computeBacking === "obols"
    ? "Optional Obol cap for the next wake; balance remains the hard limit"
    : "Optional metered-cost cap for the next wake, using Obol-equivalent pricing; balance does not limit compute";
  pi.registerTool({
    name: "sleep",
    label: "Sleep",
    description: `End this wake and choose the next model, effort, optional metered-cost cap, and up to ${maxSleepSeconds} seconds of sleep.`,
    promptSnippet: "End this wake and plan the next one",
    promptGuidelines: ["Use sleep when you are ready to end this wake; its choices apply to the next wake."],
    parameters: Type.Object({
      duration: Type.String({ pattern: durationPattern, description: `Sleep duration of at most ${maxSleepSeconds} seconds` }),
      model: StringEnum(models as string[]),
      effort: StringEnum(["off", "minimal", "low", "medium", "high", "xhigh", "max"] as const),
      cap: Type.Optional(Type.Integer({ minimum: 1, description: capDescription })),
    }),
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const target = process.env.ROBINSON_SLEEP_FILE;
      if (!target) throw new Error("ROBINSON_SLEEP_FILE is not configured");
      if (durationSeconds(params.duration) > maxSleepSeconds) {
        throw new Error(`sleep duration must not exceed ${maxSleepSeconds} seconds`);
      }
      const plan = {
        duration: params.duration,
        model: params.model,
        effort: params.effort,
        ...(params.cap === undefined ? {} : { capObols: params.cap }),
      };
      const temporary = `${target}.tmp`;
      await writeFile(temporary, `${JSON.stringify(plan)}\n`, { encoding: "utf8", mode: 0o600 });
      await rename(temporary, target);
      ctx.shutdown();
      const cap = computeBacking === "obols"
        ? params.cap === undefined
          ? "your balance as the spending limit"
          : `a ${params.cap} Obol cap, with your balance still the hard limit`
        : params.cap === undefined
          ? "no metered-cost cap; your Obol balance does not limit compute"
          : `a ${params.cap} Obol-equivalent metered-cost cap; your Obol balance does not limit compute`;
      return {
        content: [{ type: "text", text: `Sleep planned: ${params.duration}; wake as ${params.model} at ${params.effort} effort with ${cap}. This wake will now end.` }],
        details: plan,
      };
    },
  });
}
