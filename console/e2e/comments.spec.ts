import { expect, test } from "@playwright/test";

// Review comments on code: pick a range of lines, propose a change, reply. Desktop and phone.
for (const [width, height] of [[1440, 900], [390, 844]] as const) {
  test(`comment on lines of code at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await page.goto("/#/lumen/fleet/reviews");
    await page.getByRole("list", { name: "Change requests" }).getByRole("link").first().click();
    const rows = page.locator(".drow:has(.add-comment)");
    await rows.nth(3).locator(".add-comment").click();
    await rows.nth(4).locator(".dn[data-commentable]").click({ modifiers: ["Shift"] });

    const box = page.getByRole("textbox", { name: /Comment on lines/ });
    await expect(box).toBeFocused();
    await page.keyboard.type("Keep the **old** name for one release.");
    await page.getByRole("button", { name: /Suggest a change/ }).click();
    // Focus comes back to the text, so typing never reaches the page's shortcuts.
    await expect(box).toBeFocused();
    await page.keyboard.type(" // reviewed");
    await page.keyboard.press("ControlOrMeta+Enter");

    const suggestion = page.getByLabel("Suggested change");
    await expect(suggestion).toBeVisible();
    await expect(suggestion.locator('[data-op="del"]').first()).toBeVisible();
    await expect(suggestion.locator('[data-op="add"]', { hasText: "// reviewed" })).toBeVisible();
    await expect(page.locator(".comment-body strong", { hasText: "old" })).toBeVisible();
    await expect(page.locator(".comment-body pre")).toHaveCount(0);

    await page.getByRole("button", { name: "Reply…" }).first().click();
    await page.keyboard.type("Agreed.");
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect(page.locator(".comment-body", { hasText: "Agreed." })).toBeVisible();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });
}
