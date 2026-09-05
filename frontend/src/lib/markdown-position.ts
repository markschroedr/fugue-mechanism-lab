import { marked, type Token, type TokensList } from "marked";

import type { ReportProvenanceAnchor } from "./types";

export type MarkdownSourceRange = {
  start: number;
  end: number;
  quote: string;
};

type PositionedToken = Token & {
  __sourceStart?: number;
  __sourceEnd?: number;
  __outerStart?: number;
  __outerEnd?: number;
};

/**
 * Render markdown with an immutable source boundary map on every selectable text
 * run. The map uses Unicode scalar offsets, matching Rust's `str::chars()`.
 */
export function renderPositionedMarkdown(
  content: string,
  provenanceAnchors: ReportProvenanceAnchor[]
): string {
  const tokens = positionedTokens(content);
  const renderer = new marked.Renderer();
  const renderText = renderer.text.bind(renderer);
  const renderCodespan = renderer.codespan.bind(renderer);
  const renderCode = renderer.code.bind(renderer);

  renderer.html = (token) => wrapTextRun(
    token as PositionedToken,
    token.raw,
    escapeMarkdownText(token.raw),
    provenanceAnchors
  );
  renderer.text = (token) => {
    const positioned = token as PositionedToken;
    if ("tokens" in token && token.tokens) return renderer.parser.parseInline(token.tokens);
    return wrapTextRun(positioned, token.text, renderText(token), provenanceAnchors);
  };
  renderer.codespan = (token) => {
    const positioned = token as PositionedToken;
    const rendered = renderCodespan(token);
    const inner = rendered.startsWith("<code>") && rendered.endsWith("</code>")
      ? rendered.slice(6, -7)
      : escapeMarkdownText(token.text);
    return `<code>${wrapTextRun(positioned, token.text, inner, provenanceAnchors)}</code>`;
  };
  renderer.code = (token) => {
    const positioned = token as PositionedToken;
    const rendered = renderCode(token);
    const codeStart = rendered.indexOf("<code");
    const innerStart = codeStart < 0 ? -1 : rendered.indexOf(">", codeStart) + 1;
    const innerEnd = rendered.lastIndexOf("</code>");
    if (innerStart <= 0 || innerEnd < innerStart) return rendered;
    const visible = `${token.text.replace(/\n$/, "")}\n`;
    return `${rendered.slice(0, innerStart)}${wrapTextRun(positioned, visible, rendered.slice(innerStart, innerEnd), provenanceAnchors)}${rendered.slice(innerEnd)}`;
  };

  return marked.parser(tokens, { renderer, gfm: true, breaks: false });
}

/** Resolve a browser Range back to an exact raw-markdown range and quote. */
export function resolveMarkdownRange(
  root: HTMLElement,
  range: Range,
  content: string
): MarkdownSourceRange {
  const start = sourceBoundary(root, range.startContainer, range.startOffset);
  const end = sourceBoundary(root, range.endContainer, range.endOffset);
  if (start === null || end === null || end <= start) {
    throw new Error("The selected text is not an exact report span");
  }
  const quote = Array.from(content).slice(start, end).join("");
  if (!quote) throw new Error("The selected text is not an exact report span");
  return { start, end, quote };
}

function positionedTokens(content: string): TokensList {
  const tokens = marked.lexer(content, { gfm: true, breaks: false });
  let searchFrom = 0;
  for (const token of tokens as PositionedToken[]) {
    const raw = token.raw ?? "";
    const index = content.indexOf(raw, searchFrom);
    if (index < 0) continue;
    const start = scalarLength(content.slice(0, index));
    annotateToken(token, start);
    searchFrom = index + raw.length;
  }
  return tokens;
}

function annotateToken(token: PositionedToken, sourceStart: number): PositionedToken[] {
  const raw = token.raw ?? "";
  token.__sourceStart = sourceStart;
  token.__sourceEnd = sourceStart + scalarLength(raw);
  token.__outerStart = token.__sourceStart;
  token.__outerEnd = token.__sourceEnd;

  const descendants: PositionedToken[] = [];
  let searchFrom = 0;
  for (const group of childTokenGroups(token)) {
    for (const child of group) {
      const childRaw = child.raw ?? "";
      const index = raw.indexOf(childRaw, searchFrom);
      if (index < 0) continue;
      const childStart = sourceStart + scalarLength(raw.slice(0, index));
      descendants.push(...annotateToken(child, childStart));
      searchFrom = index + childRaw.length;
    }
  }

  const leaves = descendants.length > 0 ? descendants : isSelectableLeaf(token) ? [token] : [];
  if (descendants.length > 0) {
    descendants[0]!.__outerStart = sourceStart;
    descendants.at(-1)!.__outerEnd = sourceStart + scalarLength(raw);
  }
  return leaves;
}

function childTokenGroups(token: PositionedToken): PositionedToken[][] {
  const groups: PositionedToken[][] = [];
  const record = token as PositionedToken & {
    tokens?: Token[];
    items?: Array<PositionedToken & { tokens?: Token[] }>;
    header?: Array<{ tokens: Token[] }>;
    rows?: Array<Array<{ tokens: Token[] }>>;
  };
  if (record.tokens) groups.push(record.tokens as PositionedToken[]);
  if (record.items) groups.push(record.items as PositionedToken[]);
  if (record.header) groups.push(...record.header.map((cell) => cell.tokens as PositionedToken[]));
  if (record.rows) groups.push(...record.rows.flatMap((row) => row.map((cell) => cell.tokens as PositionedToken[])));
  return groups;
}

function isSelectableLeaf(token: PositionedToken): boolean {
  return token.type === "text" || token.type === "escape" || token.type === "codespan" || token.type === "code" || token.type === "html";
}

function wrapTextRun(
  token: PositionedToken,
  visibleSource: string,
  rendered: string,
  provenanceAnchors: ReportProvenanceAnchor[]
): string {
  if (token.__sourceStart === undefined || token.__sourceEnd === undefined) return rendered;
  const visible = decodeEntities(visibleSource);
  const boundaries = sourceBoundaries(token, visible);
  const attrs = `data-source-start="${token.__outerStart}" data-source-end="${token.__outerEnd}" data-source-boundaries="${boundaries.join(",")}" data-visible-codepoints="${encodeVisibleText(visible)}"`;
  if (visibleSource !== token.raw) return `<span ${attrs}>${rendered}</span>`;
  const candidates = provenanceAnchors
    .filter((anchor) => anchor.start < token.__sourceEnd! && anchor.end > token.__sourceStart!)
    .map((anchor) => ({
      anchor,
      start: Math.max(anchor.start, token.__sourceStart!) - token.__sourceStart!,
      end: Math.min(anchor.end, token.__sourceEnd!) - token.__sourceStart!
    }))
    .filter((candidate) => candidate.end > candidate.start)
    .sort((left, right) => left.start - right.start || left.end - right.end);
  if (candidates.some((candidate, index) => index > 0 && candidate.start < candidates[index - 1]!.end)) {
    return `<span ${attrs}>${rendered}</span>`;
  }

  const characters = Array.from(visibleSource);
  let cursor = 0;
  let body = "";
  for (const candidate of candidates) {
    body += escapeMarkdownText(characters.slice(cursor, candidate.start).join(""));
    body += `<span class="report-provenance" data-provenance-id="${escapeAttribute(candidate.anchor.id)}" role="button" tabindex="0">${escapeMarkdownText(characters.slice(candidate.start, candidate.end).join(""))}</span>`;
    cursor = candidate.end;
  }
  body += escapeMarkdownText(characters.slice(cursor).join(""));
  return `<span ${attrs}>${candidates.length > 0 ? body : rendered}</span>`;
}

function sourceBoundaries(token: PositionedToken, visible: string): number[] {
  const outerStart = token.__outerStart ?? token.__sourceStart ?? 0;
  const outerEnd = token.__outerEnd ?? token.__sourceEnd ?? outerStart;
  const innerStart = token.__sourceStart ?? outerStart;
  const raw = Array.from(token.raw ?? "");
  const visibleCharacters = Array.from(visible);
  if (visibleCharacters.length === 0) return [outerStart];

  let rawIndex = 0;
  const literalStart = (token.raw ?? "").indexOf(visible);
  if (literalStart >= 0) rawIndex = scalarLength((token.raw ?? "").slice(0, literalStart));
  const boundaries = [innerStart + rawIndex];
  for (const character of visibleCharacters) {
    const entityEnd = raw[rawIndex] === "&" ? raw.indexOf(";", rawIndex + 1) : -1;
    if (entityEnd > rawIndex && decodeEntities(raw.slice(rawIndex, entityEnd + 1).join("")) === character) {
      rawIndex = entityEnd + 1;
    } else if (raw[rawIndex] === "\\" && raw[rawIndex + 1] === character) {
      rawIndex += 2;
    } else if (raw[rawIndex] === character) {
      rawIndex += 1;
    } else {
      const next = raw.indexOf(character, rawIndex);
      rawIndex = next >= 0 ? next + 1 : rawIndex + 1;
    }
    boundaries.push(innerStart + rawIndex);
  }
  boundaries[0] = outerStart;
  boundaries[visibleCharacters.length] = outerEnd;
  return boundaries;
}

function sourceBoundary(root: HTMLElement, container: Node, offset: number): number | null {
  const textNode = container.nodeType === Node.TEXT_NODE
    ? container as Text
    : boundaryTextNode(container, offset);
  if (!textNode || !root.contains(textNode)) return null;
  const mapped = textNode.parentElement?.closest<HTMLElement>("[data-source-boundaries]");
  if (!mapped || !root.contains(mapped)) return null;
  const expected = mapped.dataset.visibleCodepoints;
  if (expected === undefined || encodeVisibleText(mapped.textContent ?? "") !== expected) {
    throw new Error("The rendered report no longer matches its source position map");
  }

  const prefix = document.createRange();
  prefix.selectNodeContents(mapped);
  if (container.nodeType === Node.TEXT_NODE) prefix.setEnd(container, offset);
  else prefix.setEnd(textNode, offset === 0 ? 0 : textNode.data.length);
  const visibleOffset = scalarLength(prefix.toString());
  const boundaries = (mapped.dataset.sourceBoundaries ?? "").split(",").map(Number);
  return Number.isFinite(boundaries[visibleOffset]) ? boundaries[visibleOffset]! : null;
}

function boundaryTextNode(container: Node, offset: number): Text | null {
  const children = container.childNodes;
  const candidate = offset < children.length ? children[offset] : children[offset - 1];
  if (!candidate) return null;
  if (candidate.nodeType === Node.TEXT_NODE) return candidate as Text;
  const walker = document.createTreeWalker(candidate, NodeFilter.SHOW_TEXT);
  if (offset < children.length) return walker.nextNode() as Text | null;
  let last: Text | null = null;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) last = node as Text;
  return last;
}

function scalarLength(value: string): number { return Array.from(value).length; }

/**
 * Encode visible text without attribute whitespace. DOMPurify trims attribute
 * values, so storing literal text would corrupt leading and trailing spaces.
 */
function encodeVisibleText(value: string): string {
  return Array.from(value, (character) => character.codePointAt(0)!.toString(36)).join(".");
}

function decodeEntities(value: string): string {
  if (!value.includes("&")) return value;
  if (typeof document !== "undefined") {
    const element = document.createElement("textarea");
    element.innerHTML = value;
    return element.value;
  }
  const named: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: "\u00a0" };
  return value.replace(/&(#\d+|#x[\da-f]+|\w+);/gi, (match, entity: string) => {
    if (entity[0] !== "#") return named[entity.toLowerCase()] ?? match;
    const hexadecimal = entity[1]?.toLowerCase() === "x";
    const codePoint = Number.parseInt(entity.slice(hexadecimal ? 2 : 1), hexadecimal ? 16 : 10);
    try { return String.fromCodePoint(codePoint); } catch { return match; }
  });
}

function escapeMarkdownText(value: string): string {
  return value.replace(/&(?!#\d+;|#x[\da-f]+;|\w+;)|[<>"']/gi, (character) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;"
  })[character]!);
}

function escapeAttribute(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
}
