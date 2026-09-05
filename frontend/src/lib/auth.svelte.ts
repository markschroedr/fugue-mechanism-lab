import { runtime } from "../runtime";
import type { AuthActor } from "../runtime/types";

class AuthState {
  actor = $state<AuthActor | null>(null);
  loading = $state(true);
  error = $state<string | null>(null);

  async load() {
    try {
      this.actor = await runtime.authSession();
      this.error = null;
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to check session";
    } finally {
      this.loading = false;
    }
  }

  async authenticate(mode: "login" | "register", username: string, password: string) {
    try {
      this.error = null;
      this.actor = mode === "login"
        ? await runtime.login({ username, password })
        : await runtime.register({ username, password });
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Authentication failed";
      throw error;
    }
  }

  async logout() {
    try {
      await runtime.logout();
      this.actor = null;
      this.error = null;
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to log out";
    }
  }
}

export const auth = new AuthState();
