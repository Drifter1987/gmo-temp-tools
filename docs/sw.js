// Offline shell for the hosted GMO TempTools build.
//
// Only same-origin static assets are cached. The API and every provider call is
// cross-origin and goes straight to the network, so no inbox, number or OTP data
// is ever written to the cache.

const CACHE = "gmo-temptools-shell-1795566250";
const INDEX = new URL("./index.html", self.registration.scope).href;
const SHELL = [
  "./",
  "./index.html",
  "./app.css",
  "./app.js",
  "./ui.js",
  "./config.js",
  "./manifest.webmanifest",
  "./icon-192.png",
  "./icon-512.png",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE);
      await cache.addAll(SHELL);
      await self.skipWaiting();
    })(),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const keys = await caches.keys();
      await Promise.all(
        keys.filter((key) => key !== CACHE).map((key) => caches.delete(key)),
      );
      await self.clients.claim();
    })(),
  );
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  if (request.method !== "GET") return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  // Navigations: try the network, fall back to the cached shell offline.
  if (request.mode === "navigate") {
    event.respondWith(
      (async () => {
        try {
          const response = await fetch(request);
          if (response.ok) {
            const cache = await caches.open(CACHE);
            await cache.put(INDEX, response.clone());
          }
          return response;
        } catch {
          const cache = await caches.open(CACHE);
          return (await cache.match(INDEX)) ?? Response.error();
        }
      })(),
    );
    return;
  }

  // Static shell: serve the cached copy, revalidate in the background.
  event.respondWith(
    (async () => {
      const cache = await caches.open(CACHE);
      const cached = await cache.match(request);
      const network = fetch(request)
        .then(async (response) => {
          if (response.ok) await cache.put(request, response.clone());
          return response;
        })
        .catch(() => cached);
      return cached ?? network;
    })(),
  );
});
