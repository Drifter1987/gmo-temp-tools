import { expect, type Page, type Route } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";

export function authToken(): string {
  const fromEnv = process.env.TEMPTOOLS_AUTH_TOKEN ?? process.env.E2E_AUTH_TOKEN;
  if (fromEnv?.trim()) return fromEnv.trim();
  const tokenFile = path.resolve(process.cwd(), ".run", "auth-token");
  try {
    const value = fs.readFileSync(tokenFile, "utf8").trim();
    if (value) return value;
  } catch {
    // The error below explains how to start the app and find the token.
  }
  throw new Error(
    "No app token found. Start GMO TempTools and set TEMPTOOLS_AUTH_TOKEN or .run/auth-token.",
  );
}

export async function login(page: Page): Promise<void> {
  await page.goto("/");
  if (await page.locator("#main-tabs").isVisible().catch(() => false)) return;
  await page.getByLabel("Install token").fill(authToken());
  await page.getByRole("button", { name: "Unlock" }).click();
  await expect(page.locator("#main-tabs")).toBeVisible();
}

export async function openTab(page: Page, screen: "mail" | "sms" | "otp" | "settings"): Promise<void> {
  await page.locator(`#main-tabs [data-screen="${screen}"]`).click();
}

export async function fulfillJson(route: Route, body: unknown, status = 200): Promise<void> {
  await route.fulfill({
    status,
    contentType: "application/json; charset=utf-8",
    body: JSON.stringify(body),
  });
}

export const future = (): number => Date.now() + 60 * 60 * 1000;

export async function mockMail(page: Page): Promise<void> {
  const inbox = {
    id: "e2e-inbox",
    address: "e2e-mail@example.test",
    provider: "mail.tm",
    created_at: Date.now(),
    expires_at: future(),
    unread: 1,
    last: {
      sender: "Acme <no-reply@acme.test>",
      subject: "Your Acme code",
      preview: "Use this code to finish signing in.",
      received_at: Date.now(),
    },
  };

  await page.route("**/api/mail/api/status", (route) =>
    fulfillJson(route, {
      mode: "live",
      provider: "multiple",
      inboxes: 1,
      providers: [
        {
          id: "mail.tm",
          label: "Mail.tm",
          inboxes: 1,
          address_rule: "random local part on mail.tm's active domain",
        },
        {
          id: "best-tempmail",
          label: "Best-TempMail",
          inboxes: 0,
          address_rule: "provider-assigned @best-tempmail.com",
        },
      ],
      provider_health: [],
    }),
  );

  await page.route("**/api/mail/api/acceptance", (route) =>
    fulfillJson(route, {
      provider_health: [
        {
          provider: "best-tempmail",
          list_failures: 0,
          create_failures: 2,
          last_error: "",
          last_create_error: "Best-TempMail is rate limiting us",
          last_success_at: Date.now() - 60_000,
          last_attempt_at: Date.now() - 30_000,
          error_is_current: false,
        },
      ],
    }),
  );

  await page.route(/\/api\/mail\/api\/inboxes(?:\?.*)?$/, async (route) => {
    if (route.request().method() === "POST") {
      await fulfillJson(route, {
        id: inbox.id,
        address: inbox.address,
        provider: "catchmail",
        requested_provider: "best-tempmail",
        fallback: true,
        attempts: [
          {
            provider: "best-tempmail",
            error: "Best-TempMail is rate limiting us",
          },
        ],
      });
      return;
    }
    await fulfillJson(route, { inboxes: [inbox] });
  });

  await page.route(/\/api\/mail\/api\/inboxes\/e2e-inbox\/messages(?:\?.*)?$/, (route) =>
    fulfillJson(route, {
      messages: [
        {
          id: "e2e-mail-message",
          sender: "Acme <no-reply@acme.test>",
          subject: "Your Acme code",
          preview: "Use this code to finish signing in.",
          received_at: Date.now(),
          seen: false,
        },
      ],
    }),
  );

  await page.route("**/api/mail/api/messages/e2e-mail-message", (route) =>
    fulfillJson(route, {
      id: "e2e-mail-message",
      sender: "Acme <no-reply@acme.test>",
      subject: "Your Acme code",
      text: "Your verification code is 481920. It expires soon.",
      code: "481920",
      link: "",
      received_at: Date.now(),
      has_html: false,
    }),
  );
}

export async function mockSms(page: Page, state: { addCalls: number }): Promise<void> {
  const number = {
    id: "e2e-sms-number",
    e164: "+15550100200",
    country: "united-states",
    provider: "receive-sms.io",
    created_at: Date.now(),
    expires_at: future(),
  };

  await page.route("**/api/sms/api/status", (route) =>
    fulfillJson(route, {
      mode: "live",
      numbers: 1,
      provider: "receive-sms.io",
      provider_rule:
        "receive-sms.io public page; numbers are stored in E.164 form and read by digits",
      fallback_order: ["live public page", "last good cache", "signed carrier webhook"],
      source_health: {
        provider: "receive-sms.io",
        status: "degraded",
        consecutive_failures: 2,
        last_error: "receive-sms.io returned 429",
        last_retry_count: 2,
        last_source: "cache",
      },
      webhook: { twilio: false, telnyx: false, generic: false, twilio_url_override: false },
    }),
  );

  await page.route("**/api/sms/api/countries", (route) =>
    fulfillJson(route, {
      countries: ["united-states", "united-kingdom"],
      source_health: {
        provider: "receive-sms.io",
        status: "ok",
        last_error: "",
        last_source: "live",
      },
    }),
  );

  await page.route("**/api/sms/api/countries/united-states/numbers", (route) =>
    fulfillJson(route, { country: "united-states", numbers: ["15550100200"] }),
  );

  await page.route("**/api/sms/api/numbers", async (route) => {
    if (route.request().method() === "POST") {
      const body = route.request().postDataJSON() as { country?: string; number?: string };
      if (body.country !== "united-states" || body.number !== "15550100200") {
        await fulfillJson(route, { error: "unexpected number selection" }, 422);
        return;
      }
      state.addCalls += 1;
      await fulfillJson(route, { id: number.id, e164: number.e164 });
      return;
    }
    await fulfillJson(route, { numbers: [number] });
  });

  await page.route("**/api/sms/api/source/check", (route) =>
    fulfillJson(route, {
      ok: true,
      country_count: 2,
      countries: ["united-states", "united-kingdom"],
      source_health: { provider: "receive-sms.io", status: "ok", last_error: "", last_source: "live" },
    }),
  );

  await page.route(/\/api\/sms\/api\/numbers\/e2e-sms-number\/messages(?:\?.*)?$/, (route) =>
    fulfillJson(route, {
      count: 1,
      messages: [
        {
          id: "e2e-sms-message",
          sender: "+15550001111",
          preview: "Your SMS verification code is 445566",
          body: "Your SMS verification code is 445566",
          code: "445566",
          received_at: Date.now(),
          seen: false,
        },
      ],
    }),
  );

  await page.route("**/api/sms/api/messages/e2e-sms-message", (route) =>
    fulfillJson(route, {
      id: "e2e-sms-message",
      sender: "+15550001111",
      body: "Your SMS verification code is 445566",
      code: "445566",
      received_at: Date.now(),
    }),
  );
}

export async function cleanupOtpLabels(page: Page, labels: string[]): Promise<void> {
  await page.evaluate(async (wanted) => {
    const response = await fetch("/api/otp", { cache: "no-store" });
    if (!response.ok) return;
    const payload = (await response.json()) as { secrets?: { label: string }[] };
    const secrets = payload.secrets ?? [];
    for (let index = secrets.length - 1; index >= 0; index -= 1) {
      if (!wanted.includes(secrets[index].label)) continue;
      await fetch(`/api/otp/${index}`, {
        method: "DELETE",
        headers: { "X-TempTools-CSRF": "1" },
      });
    }
  }, labels);
}
