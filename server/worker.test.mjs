// Unit tests for the hosted worker.
//
//   npm run test:worker
//
// D1 is faked with Node's built-in SQLite behind a D1-shaped adapter, so the
// worker's real SQL runs (including the ON CONFLICT counters) without a network
// or a Cloudflare account. Upstream providers are mocked per test. One database
// is shared by the whole suite because the worker caches its schema setup per
// isolate; each test scopes itself with a fresh client id.

import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import worker from "./worker.js";

const ORIGIN = "https://drifter1987.github.io";

function fakeD1() {
  const database = new DatabaseSync(":memory:");

  // Statements are compiled lazily: ensureSchema prepares a CREATE INDEX for a
  // table created earlier in the same batch, which only works once that batch
  // has actually run in order.
  const prepare = (sql) => {
    let values = [];
    let compiled = null;
    const statement = () => (compiled ??= database.prepare(sql));
    return {
      bind(...args) {
        values = args;
        return this;
      },
      async first() {
        return statement().get(...values) ?? null;
      },
      async all() {
        return { results: statement().all(...values) };
      },
      async run() {
        const result = statement().run(...values);
        return { success: true, meta: { changes: Number(result.changes ?? 0) } };
      },
    };
  };

  return {
    prepare,
    async batch(statements) {
      const results = [];
      for (const entry of statements) results.push(await entry.run());
      return results;
    },
    async exec(sql) {
      database.exec(sql);
    },
    close() {
      database.close();
    },
  };
}

const db = fakeD1();
const env = { DB: db };

let clientSeq = 0;
function freshClient() {
  clientSeq += 1;
  return `test-${process.pid}-${clientSeq}`;
}

async function call(path, init = {}, client = freshClient()) {
  const headers = new Headers(init.headers);
  headers.set("x-temptools-client", client);
  headers.set("x-temptools-csrf", "1");
  headers.set("origin", ORIGIN);
  if (init.body) headers.set("content-type", "application/json");
  return worker.fetch(new Request(`https://api.test${path}`, { ...init, headers }), env);
}

function jsonResponse(value, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}

const route = (match, reply) => ({ match, reply });

function mockUpstream(routes) {
  const calls = [];
  const original = globalThis.fetch;
  globalThis.fetch = async (input, init = {}) => {
    const url = typeof input === "string" ? input : input.url;
    const method = (init.method || "GET").toUpperCase();
    calls.push({ url, method });
    const entry = routes.find((candidate) => candidate.match(url, method));
    if (!entry) throw new Error(`unexpected upstream call: ${method} ${url}`);
    return entry.reply(url, method, init);
  };
  return {
    calls,
    restore() {
      globalThis.fetch = original;
    },
  };
}

function guerrillaCreate(address = "reader@guerrilla.test") {
  return route(
    (url) => url.includes("f=get_email_address"),
    () => jsonResponse({ email_addr: address, sid_token: "sid-reader" }),
  );
}

test("mail status lists the three hosted providers and the inbox cap", async () => {
  const response = await call("/api/mail/api/status");
  assert.equal(response.status, 200);
  const body = await response.json();
  assert.deepEqual(
    body.providers.map((provider) => provider.id),
    ["guerrilla", "best-tempmail", "tempmail.ai"],
  );
  assert.equal(body.limits.inboxes, 5);
  assert.equal(body.provider_rule, "server-side providers with automatic fallback");
});

test("the shared minute counter stops a single-client read burst", async () => {
  const client = freshClient();
  // Every 25th local request charges 25 to the shared D1 counter, whose ceiling
  // is 150. The seventh charge (request 175) is the first one over the line; the
  // 180-request memory window is deliberately shadowed by this faster guard.
  for (let index = 0; index < 174; index += 1) {
    const response = await call("/api/mail/api/status", {}, client);
    assert.equal(response.status, 200, `request ${index + 1} should pass`);
  }
  const blocked = await call("/api/mail/api/status", {}, client);
  assert.equal(blocked.status, 429);
  assert.ok(Number(blocked.headers.get("retry-after")) >= 1);
  const body = await blocked.json();
  assert.equal(body.code, "rate_limited");
  assert.equal(body.limit, 150);
  assert.equal(body.window_seconds, 60);
  assert.match(body.error, /Too many requests/);
});

test("a client may hold five inboxes and the sixth is refused", async () => {
  const client = freshClient();
  let created = 0;
  const upstream = mockUpstream([
    route((url) => url.includes("f=get_email_address"), () => {
      created += 1;
      return jsonResponse({
        email_addr: `held${created}@guerrilla.test`,
        sid_token: `sid-${created}`,
      });
    }),
  ]);
  try {
    for (let index = 0; index < 5; index += 1) {
      const response = await call(
        "/api/mail/api/inboxes",
        { method: "POST", body: JSON.stringify({ provider: "guerrilla" }) },
        client,
      );
      assert.equal(response.status, 200, `create ${index + 1} should pass`);
    }
    const blocked = await call(
      "/api/mail/api/inboxes",
      { method: "POST", body: JSON.stringify({ provider: "guerrilla" }) },
      client,
    );
    assert.equal(blocked.status, 429);
    const body = await blocked.json();
    assert.equal(body.code, "inbox_limit");
    assert.equal(body.limit, 5);
    assert.equal(created, 5, "the sixth request must not reach the provider");
  } finally {
    upstream.restore();
  }
});

test("creation is capped at six per ten minutes even after deleting", async () => {
  const client = freshClient();
  let created = 0;
  const upstream = mockUpstream([
    route((url) => url.includes("f=get_email_address"), () => {
      created += 1;
      return jsonResponse({
        email_addr: `cycle${created}@guerrilla.test`,
        sid_token: `sid-${created}`,
      });
    }),
    route((url) => url.includes("f=forget_me"), () => jsonResponse({})),
  ]);
  try {
    for (let index = 0; index < 6; index += 1) {
      const response = await call(
        "/api/mail/api/inboxes",
        { method: "POST", body: JSON.stringify({ provider: "guerrilla" }) },
        client,
      );
      assert.equal(response.status, 200, `create ${index + 1} should pass`);
      const inbox = await response.json();
      const removed = await call(`/api/mail/api/inboxes/${inbox.id}`, { method: "DELETE" }, client);
      assert.equal(removed.status, 200);
    }
    const blocked = await call(
      "/api/mail/api/inboxes",
      { method: "POST", body: JSON.stringify({ provider: "guerrilla" }) },
      client,
    );
    assert.equal(blocked.status, 429);
    const body = await blocked.json();
    assert.equal(body.code, "rate_limited");
    assert.equal(body.limit, 6);
    assert.equal(body.window_seconds, 600);
    assert.equal(created, 6, "the seventh request must not reach the provider");
  } finally {
    upstream.restore();
  }
});

test("guerrilla create, list and health map a real inbox", async () => {
  const client = freshClient();
  const upstream = mockUpstream([
    guerrillaCreate(),
    route(
      (url) => url.includes("f=get_email_list"),
      () =>
        jsonResponse({
          list: [
            {
              mail_id: "w1",
              mail_from: "no-reply@guerrillamail.com",
              mail_subject: "Welcome to Guerrilla Mail",
              mail_excerpt: "hello",
              mail_read: 0,
              mail_timestamp: 1_700_000_000,
            },
            {
              mail_id: "m1",
              mail_from: "Sender <sender@example.com>",
              mail_subject: "Your code",
              mail_excerpt: "code 481920",
              mail_read: 0,
              mail_timestamp: 1_700_000_100,
            },
          ],
        }),
    ),
  ]);
  try {
    const createdResponse = await call(
      "/api/mail/api/inboxes",
      { method: "POST", body: JSON.stringify({ provider: "guerrilla" }) },
      client,
    );
    assert.equal(createdResponse.status, 200);
    const created = await createdResponse.json();
    assert.equal(created.provider, "guerrilla");
    assert.equal(created.fallback, false);
    assert.equal(created.address, "reader@guerrilla.test");

    const list = await (await call("/api/mail/api/inboxes", {}, client)).json();
    assert.equal(list.inboxes.length, 1);
    assert.equal(list.inboxes[0].unread, 1);
    assert.equal(list.inboxes[0].last.subject, "Your code");
    assert.equal(list.inboxes[0].last.sender, "Sender <sender@example.com>");

    const messages = await (
      await call(`/api/mail/api/inboxes/${created.id}/messages`, {}, client)
    ).json();
    assert.equal(messages.count, 1);
    assert.equal(messages.messages[0].sender, "Sender <sender@example.com>");
    assert.equal(messages.messages[0].preview, "code 481920");
    assert.ok(messages.messages[0].received_at > 0);

    const health = await (await call("/api/health", {}, client)).json();
    const guerrilla = health.providers.find((provider) => provider.provider === "guerrilla");
    assert.equal(guerrilla.status, "ok");
    assert.ok(guerrilla.last_success_at > 0);
  } finally {
    upstream.restore();
  }
});

test("a failing provider falls back, records the error and recovers", async () => {
  const client = freshClient();
  let bestTempMailDown = true;
  const upstream = mockUpstream([
    route(
      (url, method) => method === "POST" && url.endsWith("/v1/inboxes"),
      () =>
        bestTempMailDown
          ? jsonResponse({ error: "upstream down" }, 500)
          : jsonResponse({ address: "recovered@best.test" }),
    ),
    guerrillaCreate("fallback@guerrilla.test"),
  ]);
  try {
    const fallbackResponse = await call(
      "/api/mail/api/inboxes",
      { method: "POST", body: JSON.stringify({ provider: "best-tempmail" }) },
      client,
    );
    assert.equal(fallbackResponse.status, 200);
    const fallback = await fallbackResponse.json();
    assert.equal(fallback.provider, "guerrilla");
    assert.equal(fallback.requested_provider, "best-tempmail");
    assert.equal(fallback.fallback, true);

    const health = await (await call("/api/health", {}, client)).json();
    const failing = health.providers.find((provider) => provider.provider === "best-tempmail");
    assert.equal(failing.status, "error");
    assert.match(failing.last_error, /Best-TempMail returned 500/);
    assert.ok(failing.last_failure_at > 0);

    const acceptance = await (await call("/api/mail/api/acceptance", {}, client)).json();
    const row = acceptance.provider_health.find(
      (provider) => provider.provider === "Best-TempMail",
    );
    assert.equal(row.error_is_current, true);
    assert.match(row.last_error, /Best-TempMail returned 500/);

    bestTempMailDown = false;
    const recoveredResponse = await call(
      "/api/mail/api/inboxes",
      { method: "POST", body: JSON.stringify({ provider: "best-tempmail" }) },
      client,
    );
    assert.equal(recoveredResponse.status, 200);
    const recovered = await recoveredResponse.json();
    assert.equal(recovered.provider, "best-tempmail");
    assert.equal(recovered.fallback, false);
    assert.equal(recovered.address, "recovered@best.test");

    const after = await (await call("/api/health", {}, client)).json();
    const healthy = after.providers.find((provider) => provider.provider === "best-tempmail");
    assert.equal(healthy.status, "ok");
    assert.equal(healthy.consecutive_failures, 0);
    const recoveredAcceptance = await (
      await call("/api/mail/api/acceptance", {}, client)
    ).json();
    assert.equal(
      recoveredAcceptance.provider_health.find(
        (provider) => provider.provider === "Best-TempMail",
      ).error_is_current,
      false,
    );
  } finally {
    upstream.restore();
  }
});

test("deleting a best-tempmail inbox calls the provider and removes the row", async () => {
  const client = freshClient();
  const upstream = mockUpstream([
    route(
      (url, method) => method === "POST" && url.endsWith("/v1/inboxes"),
      () => jsonResponse({ address: "delete-me@best.test" }),
    ),
    route(
      (url, method) => method === "DELETE" && url.includes("/v1/inboxes/"),
      () => jsonResponse({}),
    ),
  ]);
  try {
    const created = await (
      await call(
        "/api/mail/api/inboxes",
        { method: "POST", body: JSON.stringify({ provider: "best-tempmail" }) },
        client,
      )
    ).json();
    assert.equal(created.provider, "best-tempmail");

    const removed = await call(`/api/mail/api/inboxes/${created.id}`, { method: "DELETE" }, client);
    assert.equal(removed.status, 200);
    assert.ok(
      upstream.calls.some(
        (entry) =>
          entry.method === "DELETE" && entry.url.includes("/v1/inboxes/delete-me%40best.test"),
      ),
      "the provider should receive the delete",
    );

    const list = await (await call("/api/mail/api/inboxes", {}, client)).json();
    assert.equal(list.inboxes.length, 0);
    const missing = await call(`/api/mail/api/inboxes/${created.id}/messages`, {}, client);
    assert.equal(missing.status, 404);
  } finally {
    upstream.restore();
  }
});

test("tempmail.ai lists messages with its bearer token", async () => {
  const client = freshClient();
  const upstream = mockUpstream([
    route(
      (url, method) => method === "POST" && url.endsWith("/api/v1/accounts"),
      () => jsonResponse({ id: "acc-1", address: "tm@tempmail.test", password: "pw" }),
    ),
    route(
      (url, method) => method === "POST" && url.endsWith("/api/v1/token"),
      () => jsonResponse({ token: "token-1" }),
    ),
    route(
      (url, method) => method === "GET" && url.endsWith("/api/v1/messages"),
      (_url, _method, init) => {
        assert.equal(new Headers(init.headers).get("authorization"), "Bearer token-1");
        return jsonResponse({
          data: {
            items: [
              {
                id: "msg-1",
                from: "bank@example.com",
                subject: "Your code",
                preview: "use 998877",
                seen: false,
                received_at: "2026-11-24T10:00:00Z",
              },
            ],
          },
        });
      },
    ),
  ]);
  try {
    const createdResponse = await call(
      "/api/mail/api/inboxes",
      { method: "POST", body: JSON.stringify({ provider: "tempmail.ai" }) },
      client,
    );
    assert.equal(createdResponse.status, 200);
    const created = await createdResponse.json();
    assert.equal(created.provider, "tempmail.ai");
    assert.equal(created.address, "tm@tempmail.test");

    const messages = await (
      await call(`/api/mail/api/inboxes/${created.id}/messages`, {}, client)
    ).json();
    assert.equal(messages.count, 1);
    assert.equal(messages.messages[0].sender, "bank@example.com");
    assert.equal(messages.messages[0].preview, "use 998877");
    assert.ok(messages.messages[0].received_at > 0);
  } finally {
    upstream.restore();
  }
});

test("all providers failing yields a 502 that names their errors", async () => {
  const client = freshClient();
  const upstream = mockUpstream([
    route(
      (url, method) =>
        method === "POST" &&
        (url.endsWith("/v1/inboxes") ||
          url.endsWith("/api/v1/accounts") ||
          url.includes("f=get_email_address")),
      () => jsonResponse({ error: "down" }, 503),
    ),
  ]);
  try {
    const response = await call(
      "/api/mail/api/inboxes",
      { method: "POST", body: JSON.stringify({ provider: "guerrilla" }) },
      client,
    );
    assert.equal(response.status, 502);
    const body = await response.json();
    assert.match(body.error, /Mail providers failed/);
    assert.match(body.error, /guerrilla/);
    assert.match(body.error, /best-tempmail/);
  } finally {
    upstream.restore();
  }
});
