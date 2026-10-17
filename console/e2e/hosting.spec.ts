import { expect, test } from "@playwright/test";
import { HOSTING } from "./setup";

// The console against a real pandod that hosts many repositories but has no review service yet.
test.beforeEach(async ({ page }) => {
  await page.goto(`/?server=${encodeURIComponent(HOSTING)}#/`);
  await page.getByRole("button", { name: "Connect", exact: true }).click();
});

test("lists the server's repositories and opens one", async ({ page }) => {
  await expect(page.getByRole("link", { name: "lumen/fleet" })).toBeVisible();
  await expect(page.getByRole("link", { name: "lumen/web" })).toBeVisible();
  // No review service on this server: no inbox, no reviews.
  await expect(page.getByRole("link", { name: /^Inbox/ })).toHaveCount(0);
  await page.getByRole("link", { name: "lumen/fleet" }).click();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("fleet");
  await expect(page.getByRole("navigation", { name: "Repository" }).getByRole("link", { name: /Reviews/ })).toHaveCount(0);
  await page.getByRole("navigation", { name: "Repository" }).getByRole("link", { name: "Code" }).click();
  await page.getByRole("row", { name: /services/ }).getByRole("link").click();
  await page.getByRole("link", { name: "billing/" }).click();
  await page.getByRole("link", { name: "tax.rs" }).click();
  await expect(page.getByLabel("File contents")).toContainText("RATE_BPS");
});

test("each repository shows only its own history", async ({ page }) => {
  await page.getByRole("link", { name: "lumen/web" }).click();
  await page.getByRole("navigation", { name: "Repository" }).getByRole("link", { name: "Changes" }).click();
  await expect(page.getByText(/Nothing has landed on main yet/)).toBeVisible();
});

test("data from one connection never shows on another", async ({ page }) => {
  // The demo has an inbox; this server has none. After switching, nothing of the demo remains.
  await page.goto("/#/");
  await page.goto(`/?server=${encodeURIComponent(HOSTING)}#/`);
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  await expect(page.getByRole("link", { name: "lumen/fleet" })).toBeVisible();
  await expect(page.getByText("Lumen Labs")).toHaveCount(0);
  await expect(page.getByRole("link", { name: /^Inbox/ })).toHaveCount(0);
});
