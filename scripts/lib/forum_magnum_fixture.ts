import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export const NGO_POST_ID = "BBd2EJywf2xXftyFn";
export const NGO_THREAD_URL = `https://www.lesswrong.com/posts/${NGO_POST_ID}/selective-optimism-a-critique-of-ai-2040`;
export const PLAN_A_THREAD_URL = "https://www.lesswrong.com/posts/z9tXCGogEgkgHSh8G/introduction-for-and-reactions-to-plan-a";
export const SYNTHETIC_FALSIFIER_COMMENT_ID = "synthetic-falsifier-comment";
export const SYNTHETIC_FALSIFIER_QUOTE = "I would abandon this position if an independently audited frontier-model deployment caused a catastrophic incident during its first year.";
export const SYNTHETIC_FALSIFIER_CONDITION = "An independently audited frontier-model deployment causes a catastrophic incident during its first year";

const snapshotPath = join(import.meta.dir, "..", "fixtures", `lesswrong-${NGO_POST_ID}.json`);

export function startForumMagnumFixture(port: number) {
  const snapshot = JSON.parse(readFileSync(snapshotPath, "utf8"));
  const snapshotPost = snapshot.data?.post?.result;
  const snapshotComments = snapshot.data?.comments?.results;
  if (snapshotPost?._id !== NGO_POST_ID || snapshotPost.contents?.plaintextMainText?.length !== 2000 || snapshotComments?.length !== 13) {
    throw new Error("Ngo ForumMagnum fixture integrity check failed");
  }
  snapshotComments.push({
    _id: SYNTHETIC_FALSIFIER_COMMENT_ID,
    parentCommentId: null,
    postedAt: "2026-07-31T12:00:00.000Z",
    contents: {
      plaintextMainText: `Synthetic fixture statement: I support selective optimism. ${SYNTHETIC_FALSIFIER_QUOTE}`
    },
    user: {
      _id: "synthetic-falsifier-author",
      displayName: "Synthetic Falsifier Author",
      slug: "synthetic-falsifier-author"
    }
  });
  const requests: Array<{ postId: string; offset: number; limit: number }> = [];
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port,
    async fetch(request) {
      const body = await request.json() as { variables?: { postId?: string; offset?: number; limit?: number } };
      const variables = body.variables ?? {};
      const postId = variables.postId ?? "";
      const offset = variables.offset ?? 0;
      const limit = variables.limit ?? 200;
      requests.push({ postId, offset, limit });
      if (postId !== NGO_POST_ID) {
        return Response.json({ errors: [{ message: `fixture has no post ${postId}` }] });
      }
      return Response.json({
        data: {
          post: snapshot.data.post,
          comments: { results: snapshot.data.comments.results.slice(offset, offset + limit) }
        }
      });
    }
  });
  return { server, requests, endpoint: `http://127.0.0.1:${port}/graphql` };
}

export function writeFixtureConfig(path: string, endpoint: string) {
  writeFileSync(path, `[llm]\nmodel = "openai/gpt-5.6-luna"\neffort = "medium"\n\n[sources.forum_magnum]\nendpoint = "${endpoint}"\n`);
}
