import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

// WCAG 2.2 AA on every screen of the demo, in both themes.
const pages = ["/#/", "/#/inbox", "/#/lumen", "/#/lumen/fleet", "/#/lumen/fleet/code/services/billing/src/lib.rs", "/#/lumen/fleet/changes", "/#/lumen/fleet/reviews", "/#/lumen/fleet/locks", "/#/lumen/fleet/agents", "/#/lumen/fleet/settings", "/#/settings/tokens", "/#/connect", "/#/welcome"];

for (const theme of ["light", "dark"] as const) {
  test(`no WCAG 2.2 AA violations, ${theme} theme`, async ({ page }) => {
    // axe on twelve screens; shared CI runners need longer than the default 30 s.
    test.setTimeout(180_000);
    await page.addInitScript((t) => localStorage.setItem("pando.console.theme", t), theme);
    const failures: string[] = [];
    for (const path of pages) {
      await page.goto(path);
      await page.waitForLoadState("networkidle");
      await page.waitForTimeout(400);
      const r = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"]).analyze();
      for (const v of r.violations) failures.push(`${path} ${v.id} (${v.impact}): ${v.nodes.length} × ${v.nodes[0]?.target.join(" ")} — ${v.nodes[0]?.failureSummary?.split("\n")[1]?.trim()}`);
    }
    console.log(failures.join("\n"));
    expect(failures).toEqual([]);
  });
}
