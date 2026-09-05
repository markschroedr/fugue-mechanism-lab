import adapterNode from "@sveltejs/adapter-node";
import adapterStatic from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

import { resolveBuildTarget } from "./src/lib/build-target.js";

/**
 * Fugue ships one shared Svelte UI to two build targets:
 *
 * - `hosted`  -> adapter-node: a SvelteKit Node server for the hosted web
 *                deployment. It is SSR/prerender capable; the current
 *                interactive surface opts out of SSR (see src/routes/+layout.ts),
 *                so today it serves a client-rendered shell that talks to Axum
 *                over the HTTP AppRuntime. Future public read routes can opt
 *                back into SSR per route without changing the adapter.
 * - `desktop` -> adapter-static: a server-less static SPA suitable for the
 *                future Tauri shell. `index.html` is the SPA fallback and the
 *                Tauri AppRuntime adapter seam is preserved in the UI runtime.
 *
 * FUGUE_BUILD_TARGET selects the target and fails closed on unknown names; an
 * unset value resolves to `hosted` for ordinary local and dev work.
 */
const { adapter } = resolveBuildTarget(process.env.FUGUE_BUILD_TARGET, {
  hosted: () => adapterNode(),
  desktop: () => adapterStatic({ fallback: "index.html" })
});

/** @type {import('@sveltejs/kit').Config} */
export default {
  preprocess: vitePreprocess(),
  kit: {
    adapter
  }
};
