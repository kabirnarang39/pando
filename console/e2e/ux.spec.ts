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
