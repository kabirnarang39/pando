import { expect, test } from "@playwright/test";
import { SERVER } from "./setup";

// The console against a real pandod that has no platform service: one repository, no reviews.
test.beforeEach(async ({ page }) => {
  await page.goto(`/?server=${encodeURIComponent(SERVER)}#/`);
});

test("opens the server's repository and browses its files", async ({ page }) => {
  await expect(page.getByRole("heading", { level: 1 })).toContainText("127.0.0.1:7611");
  await expect(page.getByRole("link", { name: "Reviews" })).toHaveCount(0);
  await page.getByRole("link", { name: "Code" }).click();
  await page.getByRole("row", { name: /services/ }).getByRole("link").click();
  await page.getByRole("link", { name: "billing/" }).click();
  await page.getByRole("link", { name: "tax.rs" }).click();
  await expect(page.getByLabel("File contents")).toContainText("RATE_BPS: u32 = 2000");
});

test("shows what landed, with a word-level diff", async ({ page }) => {
  await page.getByRole("link", { name: "Changes" }).click();
  await page.getByRole("row", { name: /Raise the tax rate/ }).click();
  await expect(page.getByRole("heading", { name: "Raise the tax rate to 20%" })).toBeVisible();
  await expect(page.locator("del").first()).toHaveText("1900");
  await expect(page.locator("ins").first()).toHaveText("2000");
});

test("lists locked files", async ({ page }) => {
  await page.getByRole("link", { name: /Locks/ }).click();
  await expect(page.getByRole("rowheader", { name: "design.psd" })).toBeVisible();
});

test("finds a file from the command menu", async ({ page }) => {
  await page.keyboard.press("ControlOrMeta+k");
  await page.keyboard.type("tax");
  await page.getByRole("option", { name: /services\/billing\/tax\.rs/ }).click();
  await expect(page.getByLabel("File contents")).toContainText("pub fn tax");
});
