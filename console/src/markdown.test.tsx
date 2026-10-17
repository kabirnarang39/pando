import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Markdown } from "./markdown";

// Repository content is untrusted: none of these may run script or reach a dangerous URL.
describe("Markdown from repositories is inert", () => {
  const attacks = [
    "<script>window.pwned = 1</script>",
    "<img src=x onerror=\"window.pwned = 1\">",
    "[click](javascript:window.pwned=1)",
    "[click](JaVaScRiPt:window.pwned=1)",
    "[click](data:text/html,<script>window.pwned=1</script>)",
    "[click](vbscript:msgbox(1))",
    "![x](javascript:window.pwned=1)",
    "<a href=\"javascript:window.pwned=1\">x</a>",
    "<iframe src=\"https://evil.example\"></iframe>",
    "<svg onload=\"window.pwned=1\"></svg>",
  ];
  for (const text of attacks) {
    it(JSON.stringify(text), () => {
      const { container } = render(<Markdown text={text} />);
      expect(container.querySelector("script, iframe, img, svg, object, embed")).toBeNull();
      for (const a of container.querySelectorAll("a")) expect(a.getAttribute("href") ?? "").not.toMatch(/^\s*(javascript|data|vbscript):/i);
      for (const el of container.querySelectorAll("*")) for (const attr of el.getAttributeNames()) expect(attr).not.toMatch(/^on/i);
      expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
    });
  }

  it("keeps safe links working and opens external ones without opener access", () => {
    const { container } = render(<Markdown text="[docs](https://pando.dev) and [local](./README.md)" />);
    const [ext, local] = container.querySelectorAll("a");
    expect(ext!.getAttribute("href")).toBe("https://pando.dev");
    expect(ext!.getAttribute("rel")).toContain("noopener");
    expect(local!.getAttribute("href")).toBe("./README.md");
  });
});
