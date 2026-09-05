import type {
  BoardPost,
  SocietyAdminAgentDetail,
  SocietyAdminLedgerEntry,
  SocietyAdminOverview,
  SocietyAdminWakeReceipt
} from "../runtime";

export type SocietyTimelineEntry =
  | { key: string; type: "post"; timestamp: string; post: BoardPost }
  | { key: string; type: "ledger"; timestamp: string; ledger: SocietyAdminLedgerEntry }
  | {
      key: string;
      type: "wake";
      timestamp: string;
      address: string;
      wake: SocietyAdminWakeReceipt;
    };

export function buildSocietyTimeline(
  overview: SocietyAdminOverview,
  details: SocietyAdminAgentDetail[]
): SocietyTimelineEntry[] {
  const entries = new Map<string, SocietyTimelineEntry>();

  for (const post of overview.posts) {
    entries.set(`post:${post.id}`, {
      key: `post:${post.id}`,
      type: "post",
      timestamp: post.createdAt,
      post
    });
  }

  overview.ledger.forEach((ledger, index) => {
    const key = `ledger:${index}:${ledger.createdAt}`;
    entries.set(key, { key, type: "ledger", timestamp: ledger.createdAt, ledger });
  });

  for (const detail of details) {
    for (const wake of detail.wakes) {
      const key = `wake:${detail.agent.address}:${wake.wakeNumber}`;
      entries.set(key, {
        key,
        type: "wake",
        timestamp: wake.endedAt,
        address: detail.agent.address,
        wake
      });
    }
  }

  return [...entries.values()].sort((left, right) => {
    const time = Date.parse(right.timestamp) - Date.parse(left.timestamp);
    return time || left.key.localeCompare(right.key);
  });
}
