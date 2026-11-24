#!/usr/bin/env node
// Live smoke test for the deployed hosted build.
//
//   npm run smoke:hosted
//   TEMPTOOLS_PAGES_URL=... TEMPTOOLS_API_BASE=... npm run smoke:hosted
//
// It checks that the Pages bundle points at the given API, reads the health
// snapshot, then creates, lists and deletes one Mail inbox. The inbox is
// removed even when an assertion fails. This touches the real providers, so it
// spends one provider inbox per run.

import assert from "node:assert/strict";

const PAGES = (
  process.env.TEMPTOOLS_PAGES_URL || "https://drifter1987.github.io/gmo-temp-tools/"
).replace(/\/+$/, "");
const API = (
  process.env.TEMPTOOLS_API_BASE || "https://gmo-temptools-web-api-z1jsh3.freebuff.site"
).replace(/\/+$/, "");
const ORIGIN = new URL(PAGES).origin;
const CLIENT = `smoke-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;

async function request(url, init = {}) {
  const response = await fetch(url, { ...init, signal: AbortSignal.timeout(30_000) });
  const body = await response.text();
  let parsed = null;
  try {
    parsed = body ? JSON.parse(body) : null;
  } catch {
    parsed = null;
  }
  return { response, body, json: parsed };
}

function headers(extra = {}) {
  return {
    Origin: ORIGIN,
    "X-TempTools-Client": CLIENT,
    "X-TempTools-CSRF": "1",
    ...extra,
  };
}

async function main() {
  const pages = await request(PAGES);
  assert.equal(pages.response.status, 200, `Pages returned ${pages.response.status}`);
  assert.match(pages.body, /GMO TempTools/i, "Pages response is not the app");
  assert.ok(
    pages.body.includes(new URL(API).host),
    `the Pages bundle does not point at ${API}`,
  );
  console.log(`ok pages   ${PAGES}`);

  const health = await request(`${API}/api/health`, { headers: headers() });
  assert.equal(health.response.status, 200, health.body);
  assert.ok(Array.isArray(health.json?.providers), "health snapshot has no providers");
  assert.ok(health.json.providers.length >= 4, "health snapshot is missing providers");
  console.log(
    `ok health  ok=${health.json.ok} stale=${health.json.stale} providers=${health.json.providers
      .map((provider) => `${provider.provider}:${provider.status}`)
      .join(",")}`,
  );

  let createdId = "";
  try {
    const created = await request(`${API}/api/mail/api/inboxes`, {
      method: "POST",
      headers: headers({ "Content-Type": "application/json" }),
      body: JSON.stringify({ provider: "guerrilla" }),
    });
    assert.equal(created.response.status, 200, `create failed: ${created.body}`);
    const inbox = created.json;
    assert.ok(inbox?.id, "create returned no inbox id");
    assert.ok(
      typeof inbox.address === "string" && inbox.address.includes("@"),
      "create returned no address",
    );
    createdId = inbox.id;
    console.log(
      `ok create  ${inbox.address} provider=${inbox.provider} fallback=${Boolean(inbox.fallback)}`,
    );

    const listed = await request(
      `${API}/api/mail/api/inboxes/${encodeURIComponent(inbox.id)}/messages`,
      { headers: headers() },
    );
    assert.equal(listed.response.status, 200, `list failed: ${listed.body}`);
    assert.ok(Number.isInteger(listed.json?.count), "list returned no count");
    assert.ok(Array.isArray(listed.json?.messages), "list returned no messages array");
    console.log(`ok list    ${listed.json.count} message(s)`);

    const removed = await request(
      `${API}/api/mail/api/inboxes/${encodeURIComponent(inbox.id)}`,
      { method: "DELETE", headers: headers() },
    );
    assert.equal(removed.response.status, 200, `delete failed: ${removed.body}`);
    createdId = "";
    console.log("ok delete  inbox removed");

    const gone = await request(
      `${API}/api/mail/api/inboxes/${encodeURIComponent(inbox.id)}/messages`,
      { headers: headers() },
    );
    assert.equal(gone.response.status, 404, `inbox still answers with ${gone.response.status}`);
    console.log("ok gone    inbox no longer answers");
  } finally {
    if (createdId) {
      const cleanup = await request(
        `${API}/api/mail/api/inboxes/${encodeURIComponent(createdId)}`,
        { method: "DELETE", headers: headers() },
      ).catch(() => null);
      if (cleanup?.response.status === 200) {
        console.log(`cleanup   deleted ${createdId}`);
      } else {
        console.error(`cleanup   could not delete ${createdId}; it expires in an hour`);
      }
    }
  }

  console.log(`smoke ok: ${PAGES} -> ${API}`);
}

try {
  await main();
} catch (error) {
  console.error(`smoke failed: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
}
