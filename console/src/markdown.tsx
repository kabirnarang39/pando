// Markdown from a repository (READMEs, change request descriptions, comments) is untrusted. It is
// parsed to tokens and rendered as React elements: raw HTML is shown as text, never injected, and
// only http(s), mailto and relative links are followed.

import { lexer, type Token, type Tokens } from "marked";
import { Fragment, useMemo, type ReactNode } from "react";

function safeHref(href: string): string | undefined {
  const h = href.trim();
  if (/^(https?:|mailto:)/i.test(h) || /^[#./]/.test(h) || !/^[a-z][a-z0-9+.-]*:/i.test(h)) return h;
  return undefined;
}

function inline(tokens: Token[] | undefined): ReactNode {
  return tokens?.map((t, i) => <Fragment key={i}>{inlineToken(t)}</Fragment>);
}

function inlineToken(t: Token): ReactNode {
  switch (t.type) {
    case "text":
      return "tokens" in t && t.tokens ? inline(t.tokens) : (t as Tokens.Text).text;
    case "strong":
      return <strong>{inline((t as Tokens.Strong).tokens)}</strong>;
    case "em":
      return <em>{inline((t as Tokens.Em).tokens)}</em>;
    case "del":
      return <s>{inline((t as Tokens.Del).tokens)}</s>;
    case "codespan":
      return <code>{(t as Tokens.Codespan).text}</code>;
    case "br":
      return <br />;
    case "link": {
      const l = t as Tokens.Link;
      const href = safeHref(l.href);
      return href ? (
        <a href={href} target={/^https?:/i.test(href) ? "_blank" : undefined} rel="noreferrer noopener">
          {inline(l.tokens)}
        </a>
      ) : (
        inline(l.tokens)
      );
    }
    case "image": {
      const im = t as Tokens.Image;
      return <span className="muted">[image: {im.text || im.href}]</span>;
    }
    case "escape":
      return (t as Tokens.Escape).text;
    case "html":
      return (t as Tokens.HTML).text;
    default:
      return "raw" in t ? (t as { raw: string }).raw : null;
  }
}

function block(t: Token, i: number): ReactNode {
  switch (t.type) {
    case "heading": {
      const h = t as Tokens.Heading;
      const Tag = (`h${Math.min(h.depth + 1, 4)}` as "h2" | "h3" | "h4");
      return <Tag key={i}>{inline(h.tokens)}</Tag>;
    }
    case "paragraph":
      return <p key={i}>{inline((t as Tokens.Paragraph).tokens)}</p>;
    case "list": {
      const l = t as Tokens.List;
      const items = l.items.map((it, k) => (
        <li key={k}>
          {it.task && <input type="checkbox" checked={!!it.checked} readOnly aria-label={it.checked ? "done" : "not done"} />} {it.tokens.map((x, j) => (x.type === "text" ? <Fragment key={j}>{inline((x as Tokens.Text).tokens ?? [x])}</Fragment> : block(x, j)))}
        </li>
      ));
      return l.ordered ? <ol key={i}>{items}</ol> : <ul key={i}>{items}</ul>;
    }
    case "code":
      return (
        <pre key={i}>
          <code>{(t as Tokens.Code).text}</code>
        </pre>
      );
    case "blockquote":
      return <blockquote key={i}>{(t as Tokens.Blockquote).tokens.map(block)}</blockquote>;
    case "table": {
      const tb = t as Tokens.Table;
      return (
        <table key={i}>
          <thead>
            <tr>
              {tb.header.map((c, k) => (
                <th key={k}>{inline(c.tokens)}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {tb.rows.map((r, k) => (
              <tr key={k}>
                {r.map((c, j) => (
                  <td key={j}>{inline(c.tokens)}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      );
    }
    case "hr":
      return <hr key={i} />;
    case "space":
      return null;
    case "html":
      return <p key={i}>{(t as Tokens.HTML).text}</p>;
    default:
      return "raw" in t ? <p key={i}>{(t as { raw: string }).raw}</p> : null;
  }
}

export function Markdown({ text, className = "markdown" }: { text: string; className?: string }) {
  const tokens = useMemo(() => lexer(text, { gfm: true }), [text]);
  return <div className={className}>{tokens.map(block)}</div>;
}
