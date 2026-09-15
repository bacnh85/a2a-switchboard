// Minimal, XSS-safe markdown for chat bubbles: fenced code, inline code,
// bold/italic, links, headings, quotes, unordered lists. Mirrors the subset
// the 0.7.x messenger.js rendered. No HTML passthrough — everything is
// text nodes or elements we build.

import type { ComponentChildren, JSX } from "preact";

function inline(text: string, keyBase: string): ComponentChildren[] {
  const out: ComponentChildren[] = [];
  // order: code spans first (contents never reformatted), then bold/italic/links
  const re = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*\n]+\*)|(https?:\/\/[^\s<>()]+[^\s<>().,!?;:'"])/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let k = 0;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    if (m[1]) {
      out.push(
        <code key={`${keyBase}-${k++}`}>{m[1].slice(1, -1)}</code>,
      );
    } else if (m[2]) {
      out.push(<strong key={`${keyBase}-${k++}`}>{m[2].slice(2, -2)}</strong>);
    } else if (m[3]) {
      out.push(<em key={`${keyBase}-${k++}`}>{m[3].slice(1, -1)}</em>);
    } else if (m[4]) {
      out.push(
        <a key={`${keyBase}-${k++}`} href={m[4]} target="_blank" rel="noreferrer noopener">
          {m[4]}
        </a>,
      );
    }
    last = re.lastIndex;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/** Render a chat message to JSX blocks. */
export function renderMarkdown(text: string): JSX.Element {
  const lines = text.split("\n");
  const blocks: JSX.Element[] = [];
  let i = 0;
  let key = 0;

  while (i < lines.length) {
    const line = lines[i];

    // fenced code
    if (line.startsWith("```")) {
      const lang = line.slice(3).trim();
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].startsWith("```")) {
        body.push(lines[i]);
        i++;
      }
      i++; // closing fence (or EOF)
      blocks.push(
        <pre key={key++} data-lang={lang}>
          <code>{body.join("\n")}</code>
        </pre>,
      );
      continue;
    }

    // heading
    const h = /^(#{1,3})\s+(.*)/.exec(line);
    if (h) {
      const Tag = (`h${h[1].length + 3}` as "h4" | "h5" | "h6");
      blocks.push(<Tag key={key++} style="margin:4px 0 2px">{inline(h[2], `h${key}`)}</Tag>);
      i++;
      continue;
    }

    // blockquote
    if (line.startsWith("> ")) {
      const body: string[] = [];
      while (i < lines.length && lines[i].startsWith("> ")) {
        body.push(lines[i].slice(2));
        i++;
      }
      blocks.push(
        <blockquote key={key++} style="border-left:3px solid var(--border-strong);margin:4px 0;padding-left:9px;color:var(--muted)">
          {inline(body.join("\n"), `q${key}`)}
        </blockquote>,
      );
      continue;
    }

    // unordered list
    if (/^[-*]\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^[-*]\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^[-*]\s+/, ""));
        i++;
      }
      blocks.push(
        <ul key={key++} style="margin:4px 0;padding-left:20px">
          {items.map((it, j) => (
            <li key={j}>{inline(it, `l${key}-${j}`)}</li>
          ))}
        </ul>,
      );
      continue;
    }

    // paragraph (merge consecutive plain lines)
    if (line.trim() === "") {
      i++;
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && lines[i].trim() !== "" && !/^(#|```|> |\* )/.test(lines[i])) {
      para.push(lines[i]);
      i++;
    }
    blocks.push(<p key={key++} style="margin:0;white-space:pre-wrap">{inline(para.join("\n"), `p${key}`)}</p>);
  }

  return <>{blocks}</>;
}
