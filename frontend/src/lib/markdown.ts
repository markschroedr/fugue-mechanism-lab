import { marked } from "marked";
import DOMPurify from "dompurify";

import { renderPositionedMarkdown } from "./markdown-position";
import type { ReportProvenanceAnchor } from "./types";

DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "A") {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noreferrer");
  }
});

/** Model-written markdown is always sanitized before it reaches the DOM. */
export function renderMarkdown(content: string): string {
  const html = marked.parse(content, { async: false, gfm: true, breaks: false });
  return DOMPurify.sanitize(html);
}

/** Report rendering also carries the raw-markdown position/provenance map. */
export function renderReportMarkdown(content: string, anchors: ReportProvenanceAnchor[]): string {
  return DOMPurify.sanitize(renderPositionedMarkdown(content, anchors));
}
