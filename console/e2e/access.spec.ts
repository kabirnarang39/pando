import { expect, test } from "@playwright/test";
import { HOSTING } from "./setup";

// Who may use what: tokens, organisation members and repository access.

test("demo: issue a token, see it once, and revoke it", async ({ page }) => {
  await page.goto("/#/settings/tokens");
  await page.getByRole("button", { name: "New token" }).click();
  await page.getByLabel("Name").fill("ci runner");
  await page.getByLabel("Acts as").fill("agent/ci-01");
  await page.getByLabel("Repositories").fill("lumen/fleet");
  await page.getByRole("button", { name: "Create token" }).click();
  await expect(page.getByLabel("New token", { exact: true })).toContainText(/^pando_demo_[0-9a-f]{64}$/);
  const row = page.getByRole("row", { name: /ci runner/ });
  await expect(row).toContainText("agent/ci-01");
  await page.getByRole("button", { name: "Done" }).click();
  await expect(page.getByLabel("New token", { exact: true })).toHaveCount(0);
  await row.getByRole("button", { name: "Revoke ci runner" }).click();
  await row.getByRole("button", { name: "Revoke", exact: true }).click();
  await expect(page.getByRole("row", { name: /ci runner/ })).toHaveCount(0);
});

test("demo: add a member and give a team access to a repository", async ({ page }) => {
  await page.goto("/#/lumen?tab=people");
  await page.getByLabel("Login of the person to add").fill("nora@lumen.dev");
  await page.getByRole("button", { name: "Add", exact: true }).click();
  const nora = page.getByRole("row", { name: /nora/ });
  await expect(nora).toBeVisible();
  await nora.getByLabel("Role for nora@lumen.dev").selectOption({ label: "Write" });
  await expect(nora.getByLabel("Role for nora@lumen.dev")).toHaveValue(String(3));
  // The last administrator can't step down.
  await page.getByRole("row", { name: /kabir/ }).getByRole("button", { name: "Remove" }).click();
  await expect(page.getByRole("alert")).toContainText("last administrator");

  await page.goto("/#/lumen/fleet/settings");
  await page.getByLabel("Person, agent or team").fill("lumen/search");
  await page.getByRole("button", { name: "Add", exact: true }).click();
  await expect(page.getByRole("row", { name: /lumen\/search/ })).toBeVisible();
  await page.getByRole("row", { name: /lumen\/search/ }).getByRole("button", { name: "Remove" }).click();
  await expect(page.getByRole("row", { name: /lumen\/search/ })).toHaveCount(0);
});

test("server: a token issued in the console is a real platform token", async ({ page }) => {
  await page.goto(`/?server=${encodeURIComponent(HOSTING)}#/`);
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  await expect(page.getByRole("link", { name: "lumen/fleet" })).toBeVisible();
  await page.goto("/#/settings/tokens");
  await page.getByRole("button", { name: "New token" }).click();
  await page.getByLabel("Name").fill("e2e");
  await page.getByRole("button", { name: "Create token" }).click();
  await expect(page.getByLabel("New token", { exact: true })).toContainText(/^pando_[0-9a-f]{64}$/);
  await expect(page.getByRole("row", { name: /e2e/ })).toContainText("Never");
});
