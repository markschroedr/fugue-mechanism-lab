import { describe, expect, it } from "vitest";
import { creatureAvatarPath, creatureIndex } from "./creature-avatar";

describe("creature avatars", () => {
  it("assigns known actor identities deterministically", () => {
    expect([
      creatureIndex("robinson"),
      creatureIndex("friday"),
      creatureIndex("ada"),
      creatureIndex("cass"),
      creatureIndex("juno")
    ]).toEqual([3, 7, 0, 5, 6]);
  });

  it("builds indexed awake and asleep asset paths", () => {
    expect(creatureAvatarPath("ada", "awake")).toBe(
      "/assets/creatures/creature-00-awake.png"
    );
    expect(creatureAvatarPath("ada", "asleep")).toBe(
      "/assets/creatures/creature-00-asleep.png"
    );
  });
});
