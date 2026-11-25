import { devices, expect, test, type Page } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { request as httpRequest } from "node:http";
import { createServer } from "node:net";
import path from "node:path";

// Blocking service workers keeps the browser from firing its own
// beforeinstallprompt and racing the synthetic events below.
test.use({ serviceWorkers: "block" });

const API_ORIGIN = "https://gmo-temptools-web-api-z1jsh3.freebuff.site";
const PHONE = devices["Pixel 5"];

let server: ChildProcess | undefined;
let baseURL = "";
let serverLog = "";

const sleep = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

function responds(url: string): Promise<boolean> {
  return new Promise((resolve) => {
    const request = httpRequest(url, { method: "GET", timeout: 750 }, (response) => {
      response.resume();
      resolve(response.statusCode === 200);
    });
    request.on("error", () => resolve(false));
    request.on("timeout", () => {
      request.destroy();
      resolve(false);
    });
    request.end();
  });
}

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = createServer();
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", () => {
      const address = probe.address();
      if (typeof address !== "object" || address === null) {
        probe.close();
        reject(new Error("could not allocate a loopback port"));
        return;
      }
      probe.close((error) => {
        if (error) reject(error);
        else resolve(address.port);
      });
    });
  });
}

async function waitForServer(): Promise<void> {
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    if (!server || server.exitCode !== null) break;
    if (await responds(baseURL)) return;
    await sleep(100);
  }
  throw new Error(`static server did not start at ${baseURL}\n${serverLog}`);
}

async function stopServer(): Promise<void> {
  if (!server || server.exitCode !== null) return;
  server.kill();
  await Promise.race([
    new Promise<void>((resolve) => {
      if (!server || server.exitCode !== null) resolve();
      else server.once("exit", () => resolve());
    }),
    sleep(3000).then(() => {
      if (server && server.exitCode === null) server.kill("SIGKILL");
    }),
  ]);
  for (let attempt = 0; attempt < 30; attempt += 1) {
    if (!(await responds(baseURL))) return;
    await sleep(100);
  }
  throw new Error(`static server is still responding after shutdown\n${serverLog}`);
}

async function openApp(page: Page): Promise<void> {
  await page.goto(`${baseURL}/`, { waitUntil: "load" });
  await expect(page.locator("#main-tabs")).toBeVisible();
  await expect(page.locator("#app")).not.toContainText("Loading GMO TempTools");
}

async function expectNoHorizontalOverflow(page: Page): Promise<void> {
  const width = await page.evaluate(() => window.innerWidth);
  const scrollWidth = await page.evaluate(() => document.documentElement.scrollWidth);
  expect(scrollWidth).toBeLessThanOrEqual(width + 1);
}

async function fireInstallPrompt(page: Page, outcome: "accepted" | "dismissed"): Promise<void> {
  await page.evaluate((choiceOutcome) => {
    type PromptEvent = Event & {
      prompt: () => Promise<void>;
      userChoice: Promise<{ outcome: "accepted" | "dismissed"; platform: string }>;
    };
    const event = new Event("beforeinstallprompt", { cancelable: true }) as PromptEvent;
    event.prompt = async () => {};
    event.userChoice = Promise.resolve({ outcome: choiceOutcome, platform: "web" });
    window.dispatchEvent(event);
  }, outcome);
}

test.beforeAll(async () => {
  const port = await freePort();
  baseURL = `http://127.0.0.1:${port}`;
  server = spawn(
    process.execPath,
    [path.resolve(process.cwd(), "scripts", "serve-pages.mjs"), String(port)],
    { cwd: process.cwd(), stdio: ["ignore", "pipe", "pipe"] },
  );
  server.stdout?.on("data", (chunk) => {
    serverLog += chunk.toString();
  });
  server.stderr?.on("data", (chunk) => {
    serverLog += chunk.toString();
  });
  server.on("error", (error) => {
    serverLog += `${error.message}\n`;
  });
  await waitForServer();
});

test.afterAll(async () => {
  await stopServer();
});

test.describe("install button", () => {
  test("stays hidden when installation is unsupported", async ({ page }) => {
    // Without a manifest or service worker the browser has no install prompt.
    await page.route("**/manifest.webmanifest", (route) => route.abort());
    await openApp(page);
    await page.waitForTimeout(300);
    await expect(page.locator("#install-app")).toBeHidden();
  });

  test("appears when a beforeinstallprompt event is available", async ({ page }) => {
    await openApp(page);
    await fireInstallPrompt(page, "dismissed");
    const button = page.locator("#install-app");
    await expect(button).toBeVisible();
    await expect(button).toHaveText("Install app");
  });

  test("hides after dismissal and explains how to retry", async ({ page }) => {
    await openApp(page);
    await fireInstallPrompt(page, "dismissed");
    const button = page.locator("#install-app");
    await expect(button).toBeVisible();

    await button.click();

    await expect(button).toBeHidden();
    await expect(page.locator(".toast")).toContainText("Installation dismissed");
  });

  test("hides after acceptance and confirms installation", async ({ page }) => {
    await openApp(page);
    await fireInstallPrompt(page, "accepted");
    const button = page.locator("#install-app");
    await expect(button).toBeVisible();

    await button.click();

    await expect(button).toBeHidden();
    await expect(button).toHaveText("Install app");
    await expect(page.locator(".toast")).toContainText("GMO TempTools installed");
  });

  test("stays hidden after the app is installed", async ({ page }) => {
    await openApp(page);
    await fireInstallPrompt(page, "dismissed");
    const button = page.locator("#install-app");
    await expect(button).toBeVisible();

    await page.evaluate(() => window.dispatchEvent(new Event("appinstalled")));

    await expect(button).toBeHidden();
    await expect(page.locator(".toast")).toContainText("GMO TempTools installed");
  });

  test.describe("phone viewport", () => {
    test.use({
      viewport: PHONE.viewport,
      userAgent: PHONE.userAgent,
      deviceScaleFactor: PHONE.deviceScaleFactor,
      isMobile: PHONE.isMobile,
      hasTouch: PHONE.hasTouch,
      serviceWorkers: "block",
    });

    test("keeps the shell and tabs within the screen", async ({ page }) => {
      // Fail API calls locally so each screen reaches its rendered state
      // without depending on the network.
      await page.route(`${API_ORIGIN}/**`, (route) => route.abort());
      await openApp(page);

      const initial = await page.evaluate(() => ({
        width: window.innerWidth,
        scrollWidth: document.documentElement.scrollWidth,
        statusDisplay: getComputedStyle(document.querySelector("#mode-status") as Element).display,
        tabsFit: [...document.querySelectorAll(".main-tab")].every(
          (tab) => tab.getBoundingClientRect().right <= window.innerWidth + 1,
        ),
      }));
      expect(initial.width).toBeLessThan(600);
      expect(initial.scrollWidth).toBeLessThanOrEqual(initial.width + 1);
      expect(initial.statusDisplay).toBe("none");
      expect(initial.tabsFit).toBe(true);

      for (const screen of ["mail", "sms", "otp", "settings"] as const) {
        const tab = page.locator(`#main-tabs [data-screen="${screen}"]`);
        await tab.tap();
        await expect(tab).toHaveClass(/\bon\b/);
        await expect(page).toHaveURL(new RegExp(`#${screen}$`));
        await expect(page.locator("#app")).not.toContainText("Loading…");
        await expectNoHorizontalOverflow(page);
      }
    });

    test("runs the install flow with touch", async ({ page }) => {
      await page.route(`${API_ORIGIN}/**`, (route) => route.abort());
      await page.route("**/manifest.webmanifest", (route) => route.abort());
      await openApp(page);

      const button = page.locator("#install-app");
      await expect(button).toBeHidden();

      await fireInstallPrompt(page, "dismissed");
      await expect(button).toBeVisible();
      const layout = await page.evaluate(() => {
        const brand = document.querySelector(".brand")?.getBoundingClientRect();
        const install = document.querySelector("#install-app")?.getBoundingClientRect();
        return {
          width: window.innerWidth,
          brandRight: brand?.right ?? 0,
          buttonLeft: install?.left ?? 0,
          buttonRight: install?.right ?? 0,
        };
      });
      expect(layout.buttonLeft).toBeGreaterThanOrEqual(0);
      expect(layout.buttonRight).toBeLessThanOrEqual(layout.width + 1);
      expect(layout.brandRight).toBeLessThanOrEqual(layout.buttonLeft);

      await button.tap();
      await expect(button).toBeHidden();
      const toast = page.locator(".toast");
      await expect(toast).toBeVisible();
      await expect(toast).toContainText("Installation dismissed");
      const toastBox = await toast.boundingBox();
      expect(toastBox).not.toBeNull();
      expect(toastBox!.x).toBeGreaterThanOrEqual(0);
      expect(toastBox!.x + toastBox!.width).toBeLessThanOrEqual(layout.width + 1);
      await expectNoHorizontalOverflow(page);

      await openApp(page);
      await fireInstallPrompt(page, "accepted");
      await expect(button).toBeVisible();
      await button.tap();
      await expect(button).toBeHidden();
      await expect(page.locator(".toast")).toContainText("GMO TempTools installed");
    });
  });
});
