import { expect, test } from "@playwright/test";
import { SERVER } from "./setup";

// The console against a real pandod that has no platform service: one repository, no reviews.
test.beforeEach(async ({ page }) => {
  // A link to a server opens the connection screen; nothing is sent until the person confirms.
  await page.goto(`/?server=${encodeURIComponent(SERVER)}#/`);
  await expect(page.getByText(/A link asked to connect to/)).toBeVisible();
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  await expect(page.getByRole("link", { name: "Code" })).toBeVisible();
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

test("finds a file from the command menu, keeping keys typed while it opens", async ({ page }) => {
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+k");
  await page.keyboard.type("tax");
  await page.getByRole("option", { name: /services\/billing\/tax\.rs/ }).click();
  await expect(page.getByLabel("File contents")).toContainText("pub fn tax");
});

test("opens a 50,000-line file quickly and keeps its last line in the page", async ({ page }) => {
  await page.getByRole("link", { name: "Code" }).click();
  const started = Date.now();
  await page.getByRole("link", { name: "generated.rs" }).click();
  await expect(page.getByLabel("File contents")).toContainText("ITEM_0: u32 = 0");
  const opened = Date.now() - started;
  // Every line is in the document (so the browser's find works), even though off-screen
  // chunks are not laid out.
  await expect(page.locator("#L50000")).toContainText("ITEM_49999");
  // Above 1 MB the file shows uncoloured, and says so.
  await expect(page.getByText("Highlighting is off for files over 1 MB.")).toBeVisible();
  console.log(`50,000-line file opened in ${opened} ms`);
  // A laptop opens it in about 2 s; shared CI runners are about twice as slow.
  expect(opened).toBeLessThan(process.env.CI ? 6000 : 3000);
});

test("offers only what a single-repository server can do", async ({ page }) => {
  await page.keyboard.press("ControlOrMeta+k");
  const menu = page.getByRole("dialog", { name: "Command menu" });
  await expect(menu.getByRole("option", { name: /^Code/ })).toBeVisible();
  await expect(menu.getByRole("option", { name: /^Reviews/ })).toHaveCount(0);
  await expect(menu.getByRole("option", { name: /^Inbox/ })).toHaveCount(0);
});
