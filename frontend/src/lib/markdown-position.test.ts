import { describe, expect, it } from "vitest";

import { renderReportMarkdown } from "./markdown";
import { renderPositionedMarkdown, resolveMarkdownRange } from "./markdown-position";

describe("markdown source positions", () => {
  it("maps production-rendered selections within prose and across a bold holder label", () => {
    const markdown = "**Daniel Kokotajlo:** Plan A is presented as the safer path.";
    const host = document.createElement("article");
    host.innerHTML = renderReportMarkdown(markdown, []);
    const runs = Array.from(host.querySelectorAll<HTMLElement>("[data-source-boundaries]"));
    const holder = runs.find((node) => node.textContent === "Daniel Kokotajlo:")!;
    const prose = runs.find((node) => node.textContent === " Plan A is presented as the safer path.")!;

    const crossing = document.createRange();
    crossing.setStart(holder.firstChild!, 0);
    crossing.setEnd(prose.firstChild!, Array.from(" Plan A is presented").length);

    expect(resolveMarkdownRange(host, crossing, markdown)).toEqual({
      start: 0,
      end: Array.from("**Daniel Kokotajlo:** Plan A is presented").length,
      quote: "**Daniel Kokotajlo:** Plan A is presented"
    });

    const plain = document.createRange();
    plain.setStart(prose.firstChild!, Array.from(" Plan ").length);
    plain.setEnd(prose.firstChild!, Array.from(" Plan A is presented").length);

    expect(resolveMarkdownRange(host, plain, markdown)).toEqual({
      start: Array.from("**Daniel Kokotajlo:** Plan ").length,
      end: Array.from("**Daniel Kokotajlo:** Plan A is presented").length,
      quote: "A is presented"
    });

    prose.firstChild!.textContent = " Plan A was presented as the safer path.";
    expect(() => resolveMarkdownRange(host, plain, markdown)).toThrow(/no longer matches/i);
  });

  it("renders a discoverable affordance only on a backend-provided provenance span", () => {
    const markdown = "**Atlas:** Accepted position wording.";
    const start = Array.from("**Atlas:** ").length;
    const end = start + Array.from("Accepted position wording.").length;
    const html = renderReportMarkdown(markdown, [{
      id: "prov-0001",
      start,
      end,
      entityId: "entity-1",
      sources: [{ artifactId: "source-1", label: "comment", url: "https://example.test/comment" }]
    }]);

    expect(html).toContain('data-provenance-id="prov-0001"');
    expect(html).toContain('>Accepted position wording.</span>');

    const two = renderReportMarkdown("First position. Second position.", [
      { id: "prov-1", start: 0, end: 15, entityId: "entity-1", sources: [] },
      { id: "prov-2", start: 16, end: 32, entityId: "entity-2", sources: [] }
    ]);
    expect(two).toContain('data-provenance-id="prov-1"');
    expect(two).toContain('data-provenance-id="prov-2"');
  });

  it("maps repeated table cells, fenced code, named entities, and element endpoints", () => {
    const markdown = "| same | same |\n| --- | --- |\n| left | right |\n\n```txt\n© code\n```\n\n&copy;";
    const host = document.createElement("article");
    host.innerHTML = renderPositionedMarkdown(markdown, []);
    const same = Array.from(host.querySelectorAll<HTMLElement>("[data-source-boundaries]"))
      .filter((node) => node.textContent === "same");
    expect(same).toHaveLength(2);
    expect(same[0]!.dataset.sourceStart).not.toBe(same[1]!.dataset.sourceStart);

    const code = Array.from(host.querySelectorAll<HTMLElement>("[data-source-boundaries]"))
      .find((node) => node.textContent === "© code\n")!;
    const codeRange = document.createRange();
    codeRange.selectNodeContents(code);
    expect(resolveMarkdownRange(host, codeRange, markdown).quote).toBe("```txt\n© code\n```");

    const copyright = Array.from(host.querySelectorAll<HTMLElement>("[data-source-boundaries]"))
      .find((node) => node.textContent === "©" && node !== code)!;
    const entityRange = document.createRange();
    entityRange.selectNodeContents(copyright);
    expect(resolveMarkdownRange(host, entityRange, markdown).quote).toBe("&copy;");
  });

  it("resolves DOM endpoints through source-position spans and rejects modified text", () => {
    const markdown = "Before **bold** and [linked](https://example.test) after.";
    const host = document.createElement("article");
    host.innerHTML = renderPositionedMarkdown(markdown, []);
    const textNodes = Array.from(host.querySelectorAll<HTMLElement>("[data-source-boundaries]"));
    const bold = textNodes.find((node) => node.textContent === "bold")!;
    const linked = textNodes.find((node) => node.textContent === "linked")!;
    const range = document.createRange();
    range.setStart(bold.firstChild!, 0);
    range.setEnd(linked.firstChild!, linked.firstChild!.textContent!.length);

    expect(resolveMarkdownRange(host, range, markdown)).toEqual({
      start: Array.from("Before ").length,
      end: Array.from("Before **bold** and [linked](https://example.test)").length,
      quote: "**bold** and [linked](https://example.test)"
    });

    bold.firstChild!.textContent = "bald";
    expect(() => resolveMarkdownRange(host, range, markdown)).toThrow(/no longer matches/i);
  });
});
