import { expect, test } from "@playwright/test";

// The demo: the whole platform, simulated in the browser.
test("home shows what waits on you and the repositories", async ({ page }) => {
  await page.goto("/#/");
  await expect(page.getByRole("heading", { level: 1 })).toContainText(/Good (morning|afternoon|evening)|Working late/);
  await expect(page.getByRole("region", { name: "Waiting on you" }).getByRole("link").first()).toBeVisible();
  await expect(page.getByRole("link", { name: "lumen/fleet" })).toBeVisible();
});

test("a review can be approved, and the land gate explains what remains", async ({ page }) => {
  await page.goto("/#/lumen/fleet/reviews");
  await page.getByRole("list", { name: "Change requests" }).getByRole("link").first().click();
  await expect(page.getByRole("region", { name: /land/i })).toBeVisible();
  await page.keyboard.press("a");
  await page.getByRole("dialog", { name: "Your review" }).getByRole("button", { name: "Submit review" }).click();
  await expect(page.getByText(/kabir\s*approved/)).toBeVisible();
});

test("every page has no horizontal scroll at phone width", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  for (const path of ["/#/", "/#/inbox", "/#/lumen/fleet", "/#/lumen/fleet/reviews", "/#/lumen/fleet/changes", "/#/lumen/fleet/code/"]) {
    await page.goto(path);
    await page.waitForTimeout(300);
    const wide = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
    expect(wide, path).toBe(false);
  }
});
