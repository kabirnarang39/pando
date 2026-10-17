import { expect, test } from "@playwright/test";
import { HOSTING } from "./setup";

test("demo: an issue assigned to you is in the inbox; open, discuss, assign, close", async ({ page }) => {
  await page.goto("/#/inbox");
  await expect(page.getByRole("region", { name: "Assigned to you" })).toContainText("Refunds over 30 days need finance approval");

  await page.goto("/#/lumen/fleet/issues");
  await page.getByRole("button", { name: "New issue" }).click();
  const form = page.getByRole("form", { name: "New issue" });
  await form.getByLabel("Title").fill("Invoice PDF misses the tax line");
  await form.getByLabel("Description").fill("Customers in **Germany** see no VAT line.");
  await form.getByLabel("Labels").fill("billing");
  await form.getByRole("button", { name: "Open issue" }).click();
  await page.getByRole("list", { name: "Issues" }).getByRole("link", { name: /Invoice PDF misses the tax line/ }).click();
  await expect(page.getByRole("heading", { name: /Invoice PDF misses the tax line/ })).toBeVisible();
  await expect(page.locator(".comment-body strong", { hasText: "Germany" })).toBeVisible();

  await page.getByRole("textbox", { name: "Add to the discussion" }).fill("The template drops it when the rate is zero.");
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await expect(page.locator(".comment-body", { hasText: "drops it when the rate is zero" })).toBeVisible();

  await page.getByRole("button", { name: "Assign someone or an agent" }).click();
  await page.getByLabel("Assignees").fill("agent/fix-01");
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("complementary", { name: "Issue details" })).toContainText("fix-01");

  await page.getByRole("button", { name: "Close as done" }).click();
  await expect(page.locator(".pill", { hasText: "Done" })).toBeVisible();
  await page.getByRole("button", { name: "Reopen" }).click();
  await expect(page.locator(".pill", { hasText: "Open" })).toBeVisible();
});

test("server: open an issue, comment, and it survives a reload", async ({ page }) => {
  await page.goto(`/?server=${encodeURIComponent(HOSTING)}#/`);
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  await expect(page.getByRole("link", { name: "lumen/fleet" })).toBeVisible();
  await page.goto("/#/lumen/fleet/issues");
  await page.getByRole("button", { name: "New issue" }).click();
  await page.getByRole("form", { name: "New issue" }).getByLabel("Title").fill("Tax rate needs a config flag");
  await page.getByRole("form", { name: "New issue" }).getByRole("button", { name: "Open issue" }).click();
  await page.getByRole("link", { name: /Tax rate needs a config flag/ }).click();
  await page.getByRole("textbox", { name: "Add to the discussion" }).fill("Agreed, for the EU rollout.");
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await page.reload();
  await expect(page.locator(".comment-body", { hasText: "Agreed, for the EU rollout." })).toBeVisible();
  await page.getByRole("button", { name: "Close as done" }).click();
  await expect(page.locator(".pill", { hasText: "Done" })).toBeVisible();
});
