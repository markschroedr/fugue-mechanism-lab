import { describe, expect, it } from "vitest";

import { resolveBuildTarget } from "./build-target.js";

const targets = {
  hosted: () => ({ adapter: "node" }),
  desktop: () => ({ adapter: "static" })
};

describe("build target selection", () => {
  it("resolves an unset target to hosted for ordinary local and dev work", () => {
    const resolved = resolveBuildTarget(undefined, targets);
    expect(resolved.name).toBe("hosted");
    expect(resolved.adapter).toEqual({ adapter: "node" });
  });

  it("selects the explicit desktop static target", () => {
    const resolved = resolveBuildTarget("desktop", targets);
    expect(resolved.name).toBe("desktop");
    expect(resolved.adapter).toEqual({ adapter: "static" });
  });

  it("fails closed on an unknown target name and names the known targets", () => {
    expect(() => resolveBuildTarget("staging", targets)).toThrow(
      /Unknown FUGUE_BUILD_TARGET "staging"/
    );
    expect(() => resolveBuildTarget("staging", targets)).toThrow(/hosted, desktop/);
  });

  it("invokes the chosen adapter factory only", () => {
    let hostedCalls = 0;
    let desktopCalls = 0;
    resolveBuildTarget("hosted", {
      hosted: () => {
        hostedCalls += 1;
        return {};
      },
      desktop: () => {
        desktopCalls += 1;
        return {};
      }
    });
    expect(hostedCalls).toBe(1);
    expect(desktopCalls).toBe(0);
  });
});
