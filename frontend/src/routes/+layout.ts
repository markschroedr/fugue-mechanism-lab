/**
 * The interactive workspace is client-rendered against the Axum backend.
 *
 * Honest SSR boundary: the shared UI holds request-global reactive singletons
 * (auth/workspace/review/session in src/lib/*.svelte.ts), depends on browser
 * globals (`window`, selection, timers), and loads every read through the HTTP
 * AppRuntime after mount. None of that can serve a correct per-request SSR
 * response today, so we render on the client rather than fake SSR benefits.
 *
 * This is also the prerequisite for the desktop target's static SPA fallback.
 * The hosted adapter stays SSR/prerender capable; a future public read route
 * backed by committed projections may override `ssr` per route.
 */
export const ssr = false;

/** No route is prerenderable while the shell is client-only. */
export const prerender = false;
