import { expect, test } from "@playwright/test";
import { HOSTING } from "./setup";

// The console against a real pandod that hosts many repositories, with change requests.
test.beforeEach(async ({ page }) => {
  await page.goto(`/?server=${encodeURIComponent(HOSTING)}#/`);
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  // Connected once the server's repositories show; only then is the connection remembered.
  await expect(page.getByRole("link", { name: "lumen/fleet" })).toBeVisible();
});

test("lists the server's repositories and opens one", async ({ page }) => {
  await expect(page.getByRole("link", { name: "lumen/fleet" })).toBeVisible();
  await expect(page.getByRole("link", { name: "lumen/web" })).toBeVisible();
  await page.getByRole("link", { name: "lumen/fleet" }).click();
  await expect(page.getByRole("heading", { level: 1 })).toContainText("fleet");
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
  await expect(page.getByText("Lower PAYMENT_TERMS_DAYS")).toHaveCount(0);
});

test("a change request from the server: diff, a line comment, blockers", async ({ page }) => {
  await page.getByRole("link", { name: "lumen/fleet" }).click();
  await expect(page.getByRole("navigation", { name: "Repository" }).getByRole("link", { name: /Reviews/ })).toContainText("1");
  await page.getByRole("navigation", { name: "Repository" }).getByRole("link", { name: /Reviews/ }).click();
  await page.getByRole("button", { name: /^Open/ }).click();
  await page.getByRole("link", { name: /Lower the tax rate to 17%/ }).click();
  await expect(page.getByRole("heading", { name: /Lower the tax rate to 17%/ })).toBeVisible();
  await expect(page.getByText("Matches the new regulation.")).toBeVisible();
  await expect(page.locator(".drow[data-op=add]", { hasText: "1700" })).toBeVisible();
  // Comment on the changed line; it is stored on the server and survives a reload.
  await page.locator(".drow:has(.add-comment)", { hasText: "1700" }).locator(".add-comment").click();
  await page.keyboard.type("Is this the rate from January?");
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect(page.locator(".comment-body", { hasText: "Is this the rate from January?" })).toBeVisible();
  await page.reload();
  await expect(page.locator(".comment-body", { hasText: "Is this the rate from January?" })).toBeVisible();
});

test("protecting the target blocks the change request until it is approved", async ({ page }) => {
  await page.goto("/#/lumen/fleet/settings");
  await page.getByRole("button", { name: "Protect a branch" }).click();
  await page.getByRole("form", { name: "Protect a branch" }).getByRole("button", { name: "Protect" }).click();
  await expect(page.getByRole("listitem").filter({ hasText: "1 approval required" })).toBeVisible();
  await page.goto("/#/lumen/fleet/reviews/1");
  await expect(page.getByText("Needs 1 more approval")).toBeVisible();
  // The author can't approve their own change.
  await page.goto("/#/lumen/fleet/settings");
  await page.getByRole("button", { name: "Stop protecting main" }).click();
  await expect(page.getByRole("listitem").filter({ hasText: "1 approval required" })).toHaveCount(0);
});

test("open a change request from the console", async ({ page }) => {
  await page.goto("/#/lumen/fleet/reviews");
  await page.getByRole("button", { name: "New change request" }).click();
  const form = page.getByRole("form", { name: "New change request" });
  await form.getByLabel("Branch to land").selectOption("readme");
  await form.getByLabel("Title").fill("Add a README");
  await form.getByLabel("Description").fill("So newcomers know where to start.");
  await form.getByRole("button", { name: "Open change request" }).click();
  await expect(page.getByRole("heading", { name: /Add a README/ })).toBeVisible();
  await expect(page.locator(".drow[data-op=add]", { hasText: "The product monorepo." })).toBeVisible();
});

test("a webhook on the server: add, see the secret once, ping, remove", async ({ page }) => {
  await page.goto("/#/lumen/fleet/settings");
  await page.getByRole("button", { name: "Add webhook" }).click();
  const form = page.getByRole("form", { name: "Add webhook" });
  await form.getByLabel("Payload URL").fill("http://127.0.0.1:9/nowhere");
  await form.getByLabel("Reviews").check();
  await form.getByRole("button", { name: "Add webhook" }).click();
  await expect(page.getByLabel("Signing secret")).toHaveText(/^[0-9a-f]{64}$/);
  await page.getByRole("button", { name: "Done" }).click();
  const row = page.getByRole("listitem").filter({ hasText: "127.0.0.1:9/nowhere" });
  await expect(row).toContainText("review");
  await row.getByRole("button", { name: "Ping" }).click();
  await expect(page.getByRole("table", { name: /Deliveries to/ })).toContainText("ping");
  await expect(row).toContainText("No answer");
  await row.getByRole("button", { name: /Remove webhook/ }).click();
  await expect(page.getByRole("listitem").filter({ hasText: "127.0.0.1:9/nowhere" })).toHaveCount(0);
});
