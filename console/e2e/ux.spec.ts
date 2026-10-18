import { expect, test } from "@playwright/test";
import { HOSTING } from "./setup";

// Journeys from the UX pass: what a new repository, the clone button and a new token tell you.

test.describe("on a server that hosts many repositories", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(`/?server=${encodeURIComponent(HOSTING)}#/`);
    await page.getByRole("button", { name: "Connect", exact: true }).click();
    await expect(page.getByRole("link", { name: "lumen/fleet" })).toBeVisible();
  });

  test("an empty repository says how to put code in it", async ({ page }) => {
    await page.getByRole("link", { name: "lumen/web" }).click();
    const empty = page.getByRole("region", { name: "This repository is empty" });
    await expect(empty).toContainText(`pando clone ${HOSTING}/lumen/web web`);
    await expect(empty).toContainText(`--to ${HOSTING}/lumen/web`);
    await expect(page.getByRole("region", { name: "Just landed" })).toContainText("Nothing has landed on main yet");
  });

  test("an administrator creates a repository from the organisation page and lands on its setup", async ({ page }) => {
    await page.goto("/#/lumen");
    await page.getByRole("button", { name: "New repository" }).click();
    await page.getByLabel("Name", { exact: true }).fill("docs");
    await page.getByLabel("Description (optional)").fill("Handbook");
    await page.getByRole("button", { name: "Create repository" }).click();
    await expect(page.getByRole("heading", { level: 1 })).toContainText("docs");
    await expect(page.getByRole("region", { name: "This repository is empty" })).toContainText(`${HOSTING}/lumen/docs`);
  });

  test("the clone command names the repository on the server", async ({ page }) => {
    await page.getByRole("link", { name: "lumen/fleet" }).click();
    await page.getByRole("button", { name: "Clone" }).click();
    await expect(page.getByRole("dialog", { name: "Clone or mount" })).toContainText(`pando clone ${HOSTING}/lumen/fleet fleet`);
  });
});

test("a new token comes with ready-to-copy setup for the CLI and an agent", async ({ page }) => {
  await page.goto("/#/settings/tokens");
  await page.getByRole("button", { name: "New token" }).click();
  await page.getByLabel("Name").fill("claude");
  await page.getByRole("button", { name: "Create token" }).click();
  const secret = await page.getByLabel("New token").textContent();
  await expect(page.getByText(`export PANDO_TOKEN=${secret}`)).toBeVisible();
  await expect(page.getByText(/claude mcp add --transport http pando .*\/mcp/)).toContainText(`Bearer ${secret}`);
});

test("Home says when the server can't be reached, instead of loading forever", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("pando.console.connection", JSON.stringify({ kind: "server", url: "http://127.0.0.1:9", token: null, remember: true })));
  await page.goto("/#/");
  await expect(page.getByRole("alert")).toContainText("Can't reach http://127.0.0.1:9");
  await expect(page.getByRole("button", { name: "Try again" })).toBeVisible();
});

test("breadcrumbs name the Issues section and lead back to it", async ({ page }) => {
  await page.goto("/#/lumen/fleet/issues");
  const crumbs = page.getByRole("navigation", { name: "Breadcrumbs" });
  await expect(crumbs).toContainText("Issues");
  await page.getByRole("list", { name: "Issues" }).getByRole("link").first().click();
  await crumbs.getByRole("link", { name: "Issues" }).click();
  await expect(page).toHaveURL(/\/issues$/);
});

test("phone: the navigation drawer closes with Escape, and the organisation fits the screen", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/#/lumen?tab=agents");
  await page.getByRole("button", { name: "Open navigation" }).click();
  await expect(page.locator(".scrim")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator(".scrim")).toHaveCount(0);
  await page.waitForTimeout(300);
  expect(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)).toBe(false);
});

test("an expired or revoked sign-in leads back to sign-in", async ({ page }) => {
  // A server that refuses the token, as one does after it expires or is revoked.
  const refused = "http://127.0.0.1:7699";
  await page.route(`${refused}/**`, (r) =>
    r.fulfill({ status: 200, headers: { "content-type": "application/grpc-web+proto", "grpc-status": "16", "grpc-message": "token expired", "access-control-allow-origin": "*", "access-control-expose-headers": "grpc-status,grpc-message" }, body: "" }),
  );
  await page.addInitScript((url) => localStorage.setItem("pando.console.connection", JSON.stringify({ kind: "server", url, token: "pando_0000", remember: true })), refused);
  await page.goto("/#/");
  await expect(page.getByRole("alert")).toContainText("Sign in again to keep working");
  await page.getByRole("link", { name: "Sign in again" }).click();
  await expect(page).toHaveURL(/#\/connect$/);
});
