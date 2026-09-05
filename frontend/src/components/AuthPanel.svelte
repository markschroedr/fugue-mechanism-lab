<script lang="ts">
  import { auth } from "../lib/auth.svelte";

  let mode = $state<"login" | "register">("login");
  let username = $state("");
  let password = $state("");
  let busy = $state(false);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    busy = true;
    try {
      await auth.authenticate(mode, username.trim(), password);
      password = "";
    } catch {
      // auth.error is rendered below.
    } finally {
      busy = false;
    }
  }
</script>

<div class="auth-shell">
  <form onsubmit={submit} aria-label="Sign in to write">
    <span class="prompt">Sign in to write</span>
    <input bind:value={username} autocomplete="username" aria-label="Username" placeholder="username" disabled={busy} />
    <input bind:value={password} type="password" autocomplete={mode === "login" ? "current-password" : "new-password"} aria-label="Password" placeholder="password" disabled={busy} />
    <button type="submit" disabled={busy || username.trim().length < 2 || password.length < 8}>{mode === "login" ? "Log in" : "Register"}</button>
    <button class="switch" type="button" disabled={busy} onclick={() => (mode = mode === "login" ? "register" : "login")}>{mode === "login" ? "Register" : "Log in"}</button>
  </form>
  {#if auth.error}<p class="inline-error" role="status">{auth.error}</p>{/if}
</div>

<style>
  .auth-shell { position: absolute; bottom: 1.25rem; left: 50%; z-index: 15; width: min(calc(100% - 2.5rem), 42rem); transform: translateX(-50%); }
  form { display: flex; align-items: center; gap: .4rem; padding: .45rem .6rem; border: 1px solid var(--border-strong); border-radius: 14px; background: var(--panel-2); box-shadow: 0 12px 40px rgba(0,0,0,.5); }
  .prompt { padding: 0 .35rem; color: var(--muted); font-size: .75rem; white-space: nowrap; }
  input { min-width: 0; width: 8rem; min-height: 32px; padding: .35rem .5rem; }
  button { min-height: 32px; padding: .35rem .65rem; font-size: .75rem; }
  .switch { border-color: transparent; background: transparent; color: var(--muted); }
  p { margin: .35rem .6rem 0; }
  @media (max-width: 620px) { .prompt { display: none; } form { flex-wrap: wrap; } input { flex: 1; } }
</style>
