import { runtime } from "../runtime";
import type { SessionReadModel, SessionTurn, Stance } from "./types";

export type SessionAnchor = {
  blockId: string;
  title: string;
  /** The exact text span this session refers to, if it was opened from a selection. */
  quote?: string;
};

export type ClosedSession = {
  anchor: SessionAnchor;
  turns: SessionTurn[];
  stance: Stance | null;
};

/**
 * The one live session: private working memory anchored to a spot in the
 * document (I12). It renders inline while the anchor is on screen, docks
 * into the right flank when scrolled away, and ends as a commit proposal.
 */
class SessionState {
  id = $state<string | null>(null);
  anchor = $state<SessionAnchor | null>(null);
  turns = $state<SessionTurn[]>([]);
  error = $state<string | null>(null);

  /** A declared position on the anchored span, set when the session started from one. */
  stance = $state<Stance | null>(null);

  /** Kept current by the anchor probe inside the document. */
  anchorInView = $state(true);

  active = $derived(this.anchor !== null);

  /** A session is committable once it carries anything: turns or a stance. */
  committable = $derived(this.turns.length > 0 || this.stance !== null);

  async open(anchor: SessionAnchor, stance: Stance | null = null) {
    try {
      this.error = null;
      const opened = await runtime.openSession({
        anchorBlockId: anchor.blockId,
        quote: anchor.quote,
        stance: stance ?? undefined
      });
      this.#apply(opened);
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to open session";
      throw error;
    }
  }

  async say(text: string) {
    if (!this.id) return;
    try {
      this.error = null;
      this.#apply(await runtime.appendUserMessage(this.id, text));
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to send message";
      throw error;
    }
  }

  /** End the session and hand its contents to whoever turns it into a commit. */
  async close(): Promise<ClosedSession | null> {
    if (!this.id || !this.anchor) return null;
    try {
      this.error = null;
      await runtime.commitSession(this.id);
      const closed: ClosedSession = { anchor: this.anchor, turns: this.turns, stance: this.stance };
      this.#reset();
      return closed;
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to commit session";
      throw error;
    }
  }

  async refresh() {
    if (!this.id) return;
    const id = this.id;
    try {
      const refreshed = await runtime.readSession(id);
      if (this.id !== id) return;
      this.#apply(refreshed);
      this.error = null;
    } catch (error) {
      if (this.id !== id) return;
      this.error = error instanceof Error ? error.message : "Failed to refresh session";
    }
  }

  async discard() {
    try {
      this.error = null;
      if (this.id) await runtime.discardSession(this.id);
      this.#reset();
    } catch (error) {
      this.error = error instanceof Error ? error.message : "Failed to discard session";
      throw error;
    }
  }

  #reset() {
    this.id = null;
    this.anchor = null;
    this.turns = [];
    this.stance = null;
    this.anchorInView = true;
    this.error = null;
  }

  #apply(model: SessionReadModel) {
    if (model.state !== "active") {
      this.#reset();
      return;
    }
    this.id = model.id;
    this.anchor = {
      blockId: model.anchor.blockId,
      title: model.anchor.title,
      quote: model.anchor.quote ?? undefined
    };
    this.turns = model.turns;
    this.stance = model.stance ?? null;
    this.anchorInView = true;
  }
}

export const session = new SessionState();
