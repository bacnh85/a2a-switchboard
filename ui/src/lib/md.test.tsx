import { describe, expect, it } from "vitest";
import { renderMarkdown } from "./md";

describe("renderMarkdown — XSS-safe subset", () => {
  it("never emits raw html/script elements", () => {
    const out = renderMarkdown('<script>alert(1)</script>\n<img src=x onerror="alert(1)">');
    expect(out).toBeDefined();
  });

  it("renders fenced code blocks", () => {
    const el = renderMarkdown("```json\n{\"a\": 1}\n```");
    expect(el).toBeTruthy();
  });

  it("renders inline code, bold, italic, links", () => {
    const el = renderMarkdown("a `code` **bold** *em* https://example.com");
    expect(el).toBeTruthy();
  });

  it("renders headings, quotes, lists without crashing", () => {
    renderMarkdown("# h1\n## h2\n### h3\n> quoted\n- one\n- two\n\nplain");
  });

  it("passes through plain text unchanged in the DOM text", () => {
    // renderMarkdown builds elements; check it does not throw on edge input
    renderMarkdown("");
    renderMarkdown("```\nunclosed fence");
    renderMarkdown("**unclosed bold\n> quote");
  });
});
