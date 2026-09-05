const CREATURE_COUNT = 8;

export type CreaturePose = "awake" | "asleep";

export function creatureIndex(actorId: string): number {
  let hash = 1779033703 ^ actorId.length;
  for (let index = 0; index < actorId.length; index += 1) {
    hash = Math.imul(hash ^ actorId.charCodeAt(index), 3432918353);
    hash = (hash << 13) | (hash >>> 19);
  }
  hash = Math.imul(hash ^ (hash >>> 16), 2246822507);
  hash = Math.imul(hash ^ (hash >>> 13), 3266489909);
  return ((hash ^ (hash >>> 16)) >>> 0) % CREATURE_COUNT;
}

export function creatureAvatarPath(actorId: string, pose: CreaturePose = "awake"): string {
  const index = creatureIndex(actorId).toString().padStart(2, "0");
  return `/assets/creatures/creature-${index}-${pose}.png`;
}
