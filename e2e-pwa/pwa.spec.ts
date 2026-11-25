import { chromium, expect, test, type BrowserContext } from "@playwright/test";
import { spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { request as httpRequest } from "node:http";
import os from "node:os";
import path from "node:path";

const API_ORIGIN = "https://gmo-temptools-web-api-z1jsh3.freebuff.site";
const PORT = Number(process.env.PWA_PORT ?? 4173);
const BASE_URL = `http://127.0.0.1:${PORT}`;

// Every same-origin file the service worker must cache for the offline shell.
// If a future build stops emitting or caching one of these, a cold offline
// launch would break even though the page still looks fine online.
const SHELL_ASSETS = [
  "/index.html",
  "/app.css",
  "/app.js",
  "/ui.js",
  "/config.js",
  "/manifest.webmanifest",
  "/icon-192.png",
  "/icon-512.png",
];

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

test("the hosted bundle stays installable and boots from the offline shell", async ({
  request,
}) => {
  // The test owns the static server so it can take the server down for the
  // offline reload. Page-level CDP offline emulation alone can leave the
  // service worker able to fetch, which would not prove offline support.
  if (await responds(BASE_URL)) {
    throw new Error(`refusing to test against an existing server at ${BASE_URL}`);
  }
  const server = spawn(
    process.execPath,
    [path.resolve(process.cwd(), "scripts", "serve-pages.mjs"), String(PORT)],
    { cwd: process.cwd(), stdio: ["ignore", "pipe", "pipe"] },
  );
  let serverLog = "";
  server.stdout?.on("data", (chunk) => {
    serverLog += chunk.toString();
  });
  server.stderr?.on("data", (chunk) => {
    serverLog += chunk.toString();
  });
  server.on("error", (error) => {
    serverLog += `${error.message}\n`;
  });

  const waitForServer = async (): Promise<void> => {
    const deadline = Date.now() + 15_000;
    while (Date.now() < deadline) {
      if (server.exitCode !== null) break;
      if (await responds(BASE_URL)) return;
      await sleep(100);
    }
    throw new Error(`static server did not start at ${BASE_URL}\n${serverLog}`);
  };

  const stopServer = async (): Promise<void> => {
    if (server.exitCode !== null) return;
    server.kill();
    await Promise.race([
      new Promise<void>((resolve) => {
        if (server.exitCode !== null) resolve();
        else server.once("exit", () => resolve());
      }),
      sleep(3000).then(() => {
        if (server.exitCode === null) server.kill("SIGKILL");
      }),
    ]);
    for (let attempt = 0; attempt < 30; attempt += 1) {
      if (!(await responds(BASE_URL))) return;
      await sleep(100);
    }
    throw new Error(`static server is still responding after shutdown\n${serverLog}`);
  };

  // Chrome refuses to test installability in an incognito browser context.
  // Playwright's page fixture is always incognito, so use a disposable
  // persistent profile for this browser-level check.
  const userDataDir = await mkdtemp(path.join(os.tmpdir(), "gmo-pwa-e2e-"));
  let context: BrowserContext | undefined;

  try {
    await waitForServer();

    const channel = process.env.PLAYWRIGHT_CHANNEL;
    context = await chromium.launchPersistentContext(userDataDir, {
      baseURL: BASE_URL,
      ...(channel ? { channel } : {}),
      headless: true,
      viewport: { width: 1280, height: 900 },
    });
    const page = context.pages()[0] ?? (await context.newPage());

    // The PWA contract is about the shell, not the live API. Fail hosted calls
    // locally so the test is deterministic and exercises the API-down path too.
    await page.route(`${API_ORIGIN}/**`, (route) => route.abort());

    await page.goto("/", { waitUntil: "load" });
    await expect(page.locator("#main-tabs")).toBeVisible();
    // app.js has booted in hosted mode; Mail itself is unavailable because the
    // hosted API is deliberately blocked above.
    await expect(page.locator("#mode-status")).toContainText("hosted web");
    await expect(page.locator("#app")).not.toContainText("Loading GMO TempTools");

    // 1. A service worker must be active, controlling this page, and have
    //    populated the shell cache with every required asset.
    const worker = await page.evaluate(async () => {
      const registration = await navigator.serviceWorker.ready;
      if (!navigator.serviceWorker.controller) {
        await new Promise<void>((resolve) => {
          navigator.serviceWorker.addEventListener("controllerchange", () => resolve(), {
            once: true,
          });
        });
      }
      const keys = await caches.keys();
      const shellKey = keys.find((key) => key.startsWith("gmo-temptools-shell-")) ?? null;
      const entries = shellKey
        ? (await (await caches.open(shellKey)).keys()).map((entry) => new URL(entry.url).pathname)
        : [];
      return {
        active: registration.active?.state ?? null,
        controlled: Boolean(navigator.serviceWorker.controller),
        scope: registration.scope,
        shellKey,
        entries,
      };
    });

    expect(worker.active).toBe("activated");
    expect(worker.controlled).toBe(true);
    expect(worker.scope).toBe(`${new URL(page.url()).origin}/`);
    expect(worker.shellKey).toBeTruthy();
    for (const asset of SHELL_ASSETS) {
      expect(worker.entries).toContain(asset);
    }

    // 2. The manifest must still satisfy the install criteria: no manifest or
    //    installability errors, standalone display, and both required icons.
    const manifestResponse = await request.get("/manifest.webmanifest");
    expect(manifestResponse.ok()).toBe(true);
    const manifest = (await manifestResponse.json()) as {
      display?: string;
      icons?: { sizes?: string }[];
    };
    expect(manifest.display).toBe("standalone");
    expect(manifest.icons?.map((icon) => icon.sizes)).toEqual(
      expect.arrayContaining(["192x192", "512x512"]),
    );

    const cdp = await context.newCDPSession(page);
    const appManifest = await cdp.send("Page.getAppManifest");
    expect(appManifest.errors).toEqual([]);
    const installability = await cdp.send("Page.getInstallabilityErrors");
    expect(installability.installabilityErrors).toEqual([]);

    // 3. Stop the real server first. The CDP offline flag below only makes
    //    navigator.onLine and the banner report offline; the dead server is
    //    what guarantees the worker cannot fetch the shell over the network.
    await stopServer();
    await cdp.send("Network.enable");
    await cdp.send("Network.emulateNetworkConditions", {
      offline: true,
      latency: 0,
      downloadThroughput: 0,
      uploadThroughput: 0,
    });
    try {
      await page.reload({ waitUntil: "load" });
      await expect(page.locator("#main-tabs")).toBeVisible();
      await expect(page.locator("#offline-banner")).toBeVisible();

      await page.locator('#main-tabs [data-screen="otp"]').click();
      await expect(page.getByRole("heading", { name: "OTP", exact: true })).toBeVisible();
      await page.locator('.otp-tab[data-tab="generate"]').click();
      await page.locator("#otp-uri").fill("otpauth://totp/OfflineCheck?secret=JBSWY3DPEHPK3PXP");
      await page.locator('#otp-add button[type="submit"]').click();
      await expect(page.locator("#otp-secrets .secret-code")).toHaveText(/^\d{6}$/);
    } finally {
      await cdp.send("Network.emulateNetworkConditions", {
        offline: false,
        latency: 0,
        downloadThroughput: -1,
        uploadThroughput: -1,
      });
    }
  } finally {
    try {
      await context?.close();
    } finally {
      await stopServer();
      await rm(userDataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
    }
  }
});
