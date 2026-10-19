import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { FRESH } from "./setup";

// Signing in, and setting up an organisation from nothing: name it, invite people, connect an agent.

const providers = { providers: [{ id: "google", name: "Google", url: "/login/oidc/google" }], email: true };

async function axe(page: Page) {
  const r = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"]).analyze();
  return r.violations.map((v) => `${v.id}: ${v.nodes[0]?.target.join(" ")}`);
}

test("a server with sign-in pages offers its providers and an email link instead of a token", async ({ page }) => {
  await page.route("**/login/providers", (r) => r.fulfill({ json: providers }));
  let turnstile = false;
  await page.route("https://challenges.cloudflare.com/**", (r) => ((turnstile = true), r.abort()));
  await page.goto("/#/connect");
  const google = page.getByRole("link", { name: "Sign in with Google" });
  await expect(google).toHaveAttribute("href", "http://127.0.0.1:4173/login/oidc/google?next=/");
  await expect(page.getByLabel("Email address")).toBeVisible();
  await expect(page.getByRole("button", { name: "Email me a sign-in link" })).toBeVisible();
  await expect(page.getByRole("textbox", { name: "Token" })).toHaveCount(0);
  expect(turnstile, "no third-party script without a site key").toBe(false);
  await page.getByRole("button", { name: "Use a token instead" }).click();
  await expect(page.getByLabel("Server address")).toBeVisible();
  await expect(page.getByRole("textbox", { name: "Token" })).toBeVisible();
});

test("Turnstile loads only when the server asks for it", async ({ page }) => {
  await page.route("**/login/providers", (r) => r.fulfill({ json: { ...providers, turnstile_site_key: "1x00000000000000000000AA" } }));
  const asked = page.waitForRequest("https://challenges.cloudflare.com/turnstile/v0/api.js");
  await page.route("https://challenges.cloudflare.com/**", (r) => r.abort());
  await page.goto("/#/connect");
  await asked;
  await expect(page.locator(".cf-turnstile")).toHaveAttribute("data-sitekey", "1x00000000000000000000AA");
});

test("the session handed over after sign-in is kept and wiped from the address", async ({ page }) => {
  await page.goto("/#pando_session=pando_0123456789abcdef");
  // The fragment is replaced with the home route (where the app then goes on as usual).
  await expect.poll(() => page.evaluate(() => location.hash)).toMatch(/^#\/(?!.*pando_session)/);
  const saved = await page.evaluate(() => JSON.parse(localStorage.getItem("pando.console.connection") ?? "null"));
  expect(saved).toEqual({ kind: "server", url: "http://127.0.0.1:4173", token: "pando_0123456789abcdef", remember: true });
  // Nothing about the token stays in history.
  expect(await page.evaluate(() => location.href)).not.toContain("pando_session");
});

test("server: a person with no organisation is guided from naming one to connecting an agent", async ({ page }) => {
  await page.goto(`/?server=${encodeURIComponent(FRESH)}#/`);
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  // No organisations yet: straight into step 1.
  await expect(page.getByRole("heading", { name: "Welcome to Grovedock" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Name your organisation" })).toBeVisible();

  const name = page.getByLabel("Short name, used in addresses");
  await name.fill("not valid!");
  await page.getByRole("button", { name: "Create organisation" }).click();
  await expect(page.getByRole("alert")).toContainText("Names use letters, digits");
  await name.fill("acme");
  await page.getByLabel("Organisation name").fill("Acme Labs");
  await page.getByRole("button", { name: "Create organisation" }).click();

  await expect(page.getByRole("heading", { name: "Invite teammates" })).toBeVisible();
  await page.getByRole("button", { name: "Skip for now" }).click();

  await expect(page.getByRole("heading", { name: "Connect an agent" })).toBeVisible();
  await page.getByLabel("Agent name").fill("claude-01");
  await page.getByRole("button", { name: "Create token" }).click();
  await expect(page.getByLabel("Token", { exact: true })).toContainText(/^pando_[0-9a-f]{64}$/);
  const command = page.getByLabel("Command", { exact: true });
  await expect(command).toContainText(`claude mcp add --transport http pando ${FRESH}/mcp`);
  await expect(command).toContainText('--header "Authorization: Bearer pando_');
  await page.getByRole("button", { name: "Go to acme" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Acme Labs" })).toBeVisible();

  // The same name again is refused, in words.
  await page.getByRole("link", { name: "New organisation" }).click();
  await expect(page.getByRole("heading", { name: "New organisation", level: 1 })).toBeVisible();
  await page.getByLabel("Short name, used in addresses").fill("acme");
  await page.getByRole("button", { name: "Create organisation" }).click();
  await expect(page.getByRole("alert")).toContainText("“acme” is already taken");
});

test("demo: invite a teammate during set-up, then revoke the invitation", async ({ page }) => {
  await page.goto("/#/");
  await page.getByRole("link", { name: "New organisation" }).click();
  await page.getByLabel("Short name, used in addresses").fill("acme");
  await page.getByRole("button", { name: "Create organisation" }).click();
  await page.getByLabel("Email address to invite").fill("ana@example.com");
  await page.getByLabel("Role for the invited person").selectOption({ label: "Maintain" });
  await page.getByRole("button", { name: "Invite", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "Send this link to ana@example.com" })).toBeVisible();
  await expect(page.getByLabel("Invitation link")).toContainText(/\/login\/invite\/[0-9a-f]{64}$/);
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("radio", { name: "Me" }).check();
  await page.getByRole("button", { name: "Create token" }).click();
  await expect(page.getByLabel("Token", { exact: true })).toContainText(/^pando_demo_/);
  await page.getByRole("button", { name: "Go to acme" }).click();
  await page.getByRole("button", { name: /People/ }).click();
  const pending = page.getByRole("table", { name: "Pending invitations" });
  await expect(pending.getByRole("row", { name: /ana@example.com/ })).toContainText("Maintain");
  await pending.getByRole("button", { name: "Revoke the invitation for ana@example.com" }).click();
  await expect(page.getByRole("table", { name: "Pending invitations" })).toHaveCount(0);
});

for (const theme of ["light", "dark"] as const) {
  test(`sign-in and set-up screens have no WCAG 2.2 AA violations, ${theme} theme`, async ({ page }) => {
    test.setTimeout(90_000);
    await page.addInitScript((t) => localStorage.setItem("pando.console.theme", t), theme);
    await page.route("**/login/providers", (r) => r.fulfill({ json: providers }));
    await page.goto("/#/connect");
    await expect(page.getByRole("link", { name: "Sign in with Google" })).toBeVisible();
    const failures = await axe(page);
    // The steps after naming the organisation, with an invitation link and a token shown.
    await page.unroute("**/login/providers");
    await page.goto("/#/welcome");
    await page.getByLabel("Short name, used in addresses").fill("acme");
    await page.getByRole("button", { name: "Create organisation" }).click();
    await page.getByLabel("Email address to invite").fill("ana@example.com");
    await page.getByRole("button", { name: "Invite", exact: true }).click();
    await expect(page.getByLabel("Invitation link")).toBeVisible();
    failures.push(...(await axe(page)));
    await page.getByRole("button", { name: "Continue" }).click();
    await page.getByRole("button", { name: "Create token" }).click();
    await expect(page.getByLabel("Token", { exact: true })).toBeVisible();
    failures.push(...(await axe(page)));
    await page.getByRole("button", { name: "Go to acme" }).click();
    await page.getByRole("button", { name: /People/ }).click();
    await expect(page.getByRole("table", { name: "Pending invitations" })).toBeVisible();
    failures.push(...(await axe(page)));
    expect(failures).toEqual([]);
  });
}
