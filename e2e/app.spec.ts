import { expect, test } from "@playwright/test";
import { cleanupOtpLabels, login, mockMail, mockSms, openTab } from "./helpers";

test("all four main tabs load through the live local app", async ({ page }) => {
  await login(page);

  for (const screen of ["mail", "sms", "otp", "settings"] as const) {
    await openTab(page, screen);
    const heading = screen === "mail" ? "Mail" : screen === "sms" ? "SMS Beta" : screen === "otp" ? "OTP" : "Settings";
    await expect(page.getByRole("heading", { name: heading, exact: false })).toBeVisible();
    await expect(page.locator("#app")).not.toContainText("is unavailable");
  }
});

test("the install token and CSRF header gate browser mutations", async ({ page }) => {
  const label = `E2E CSRF ${Date.now()}`;
  const input = "otpauth://totp/Acme:csrf-check?secret=JBSWY3DPEHPK3PXP&issuer=Acme";

  await login(page);

  // There is no logout endpoint; locking the web app means dropping the
  // HttpOnly install-token cookie, as closing the browser session would.
  await page.context().clearCookies({ name: "temptools_token" });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Unlock the local app" })).toBeVisible();

  const noToken = await page.evaluate(
    async (mutation) => {
      const response = await fetch("/api/otp", {
        method: "POST",
        credentials: "same-origin",
        headers: {
          "Content-Type": "application/json",
          "X-TempTools-CSRF": "1",
        },
        body: JSON.stringify(mutation),
      });
      const payload = (await response.json()) as { error?: string };
      return { status: response.status, error: payload.error };
    },
    { input, label },
  );
  expect(noToken.status).toBe(401);
  expect(noToken.error).toBe("A per-install token is required");

  await login(page);

  try {
    const noCsrf = await page.evaluate(
      async (mutation) => {
        const response = await fetch("/api/otp", {
          method: "POST",
          credentials: "same-origin",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(mutation),
        });
        const payload = (await response.json()) as { error?: string };
        return { status: response.status, error: payload.error };
      },
      { input, label },
    );
    expect(noCsrf.status).toBe(403);
    expect(noCsrf.error).toBe("Missing CSRF header");

    const after = await page.evaluate(async () => {
      const response = await fetch("/api/otp", {
        cache: "no-store",
        credentials: "same-origin",
      });
      const payload = (await response.json()) as { secrets?: { label: string }[] };
      return {
        status: response.status,
        labels: (payload.secrets ?? []).map((secret) => secret.label),
      };
    });
    expect(after.status).toBe(200);
    expect(after.labels).not.toContain(label);
  } finally {
    await cleanupOtpLabels(page, [label]);
  }
});

test("mail tab renders provider rules, health, fallback and a message", async ({ page }) => {
  await mockMail(page);
  await login(page);

  await expect(page.getByRole("heading", { name: "Mail", exact: true })).toBeVisible();
  await expect(page.locator("#mail-provider-note")).toContainText("Address rule:");
  await expect(page.locator("#mail-health")).toContainText("2 create / 0 list failures");

  await page.locator("#mail-list .item").click();
  await expect(page.locator("#mail-thread")).toContainText("Your Acme code");
  await expect(page.locator("#mail-list .pill")).toHaveText("1");
  await page.locator("#mail-thread .item").filter({ hasText: "Your Acme code" }).click();
  await expect(page.locator("#mail-thread")).toContainText("481920");
  await expect(page.locator("#mail-list .pill")).toHaveCount(0);

  await page.locator('[data-action="mail-new"]').click();
  await expect(page.locator(".toast")).toContainText("Fell back from best-tempmail to catchmail");
});

test("sms tab shows cached source health and a delivered text", async ({ page }) => {
  const state = { addCalls: 0 };
  await mockSms(page, state);
  await login(page);
  await openTab(page, "sms");

  await expect(page.getByRole("heading", { name: /SMS/, exact: false })).toBeVisible();
  await expect(page.locator("#sms-live")).toHaveText("receive-sms.io · degraded");
  await expect(page.locator("#sms-live")).toHaveAttribute("title", /429/);
  await expect(page.locator("#sms-list")).toContainText("+15550100200");
  await expect(page.locator("#sms-number option").first()).toHaveAttribute("value", "15550100200");

  await page.locator("#sms-list .item").click();
  await expect(page.locator("#sms-thread")).toContainText("Your SMS verification code is 445566");
  await page.locator("#sms-thread .item").filter({ hasText: "verification code" }).click();
  await expect(page.locator("#sms-thread")).toContainText("445566");

  await page.locator('#sms-picker button[type="submit"]').click();
  await expect.poll(() => state.addCalls).toBe(1);
});

test("settings toggles notifications and sends a test toast", async ({ page }) => {
  const saved = {
    appearance: "dark",
    notifications: { enabled: true, mail: true, sms: true },
    owner: "drifter1987",
    version: "0.3.0",
    paths: [],
    release_notes: [],
  };
  await page.route("**/api/settings", async (route) => {
    if (route.request().method() === "POST") {
      const patch = route.request().postDataJSON() as {
        appearance?: string;
        notifications?: { enabled: boolean; mail: boolean; sms: boolean };
      };
      if (patch.notifications) saved.notifications = patch.notifications;
      if (patch.appearance === "light" || patch.appearance === "dark") saved.appearance = patch.appearance;
    }
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(saved) });
  });
  await page.route("**/api/notifications/test", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: '{"ok":true}' }),
  );
  await login(page);
  await openTab(page, "settings");

  await expect(page.getByRole("heading", { name: "Settings", exact: true })).toBeVisible();
  await expect(page.locator("#notify-enabled")).toBeChecked();
  await page.locator("#notify-mail").uncheck();
  await page.locator('[data-action="settings-test"]').click();
  await expect(page.locator(".toast")).toContainText("Test notification sent");
  await page.locator("#notify-mail").check();
});

test("otp tab extracts codes and reads the shared vault written by another client", async ({
  page,
}) => {
  const label = `E2E browser ${Date.now()}`;
  const externalLabel = `${label} API`;

  await login(page);
  await openTab(page, "otp");
  await expect(page.getByRole("heading", { name: "OTP", exact: true })).toBeVisible();

  await page.locator('[data-action="otp-tab"][data-tab="extract"]').click();
  await page.locator("#otp-text").fill("Your verification code is 481920. It expires soon.");
  await page.locator('[data-action="otp-extract"]').click();
  await expect(page.locator("#otp-found")).toContainText("481920");

  try {
    await page.locator('[data-action="otp-tab"][data-tab="generate"]').click();
    await page.locator("#otp-uri").fill("otpauth://totp/Acme:alice?secret=JBSWY3DPEHPK3PXP&issuer=Acme");
    await page.locator("#otp-label").fill(label);
    await page.locator('#otp-add button[type="submit"]').click();
    await expect(page.locator("#otp-secrets")).toContainText(label);

    // This is the same vault read by the desktop tab; a second HTTP client
    // writes through /api/otp and the browser sees the change after a reload.
    const status = await page.evaluate(async (external) => {
      const response = await fetch("/api/otp", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-TempTools-CSRF": "1",
        },
        body: JSON.stringify({
          input: "otpauth://totp/Acme:bob?secret=GEZDGNBVGY3TQOJQ&issuer=Acme",
          label: external,
        }),
      });
      return response.status;
    }, externalLabel);
    expect(status).toBe(200);

    await page.goto("/#otp");
    await page.locator('[data-action="otp-tab"][data-tab="generate"]').click();
    await expect(page.locator("#otp-secrets")).toContainText(label);
    await expect(page.locator("#otp-secrets")).toContainText(externalLabel);
  } finally {
    await cleanupOtpLabels(page, [label, externalLabel]);
  }
});
