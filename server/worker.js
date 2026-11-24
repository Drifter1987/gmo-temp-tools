// Serverless API for the hosted GMO TempTools web app.
//
// Guerrilla Mail and receive-sms.io are fetched server-side so the browser never
// has to deal with provider CORS rules. Everything stored here is public
// temporary inbox/number metadata, scoped to a random browser client id. OTP
// secrets are intentionally never sent to this service.
//
// Mail.tm is deliberately not offered here: its edge rejects requests from
// Cloudflare Workers with 403, so every Mail.tm call used to fall back. The
// desktop app still supports Mail.tm; the read helpers below stay only so any
// Mail.tm inbox created before this change can still be listed and deleted.

const VERSION = "0.3.0";
const MAIL_TTL_MS = 60 * 60 * 1000;
const SMS_TTL_MS = 7 * 24 * 60 * 60 * 1000;
const MAIL_BASE = "https://api.mail.tm";
const SMS_BASE = "https://receive-sms.io";
const SMS_AGENT =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";
const ALLOWED_ORIGINS = new Set([
  "https://drifter1987.github.io",
  "https://Drifter1987.github.io",
  "http://127.0.0.1:8780",
  "http://localhost:8780",
  "http://127.0.0.1:4173",
  "http://localhost:4173",
]);

let schemaReady = false;
let countryCache = null;
let smsHealth = {
  provider: "receive-sms.io",
  status: "unknown",
  consecutive_failures: 0,
  last_error: "",
  last_retry_count: 0,
  last_source: "live",
  last_attempt_at: 0,
  last_success_at: 0,
  last_failure_at: 0,
};

function now() {
  return Date.now();
}

function corsHeaders(origin) {
  const headers = {
    "Access-Control-Allow-Methods": "GET,POST,DELETE,OPTIONS",
    "Access-Control-Allow-Headers":
      "Content-Type, X-TempTools-Client, X-TempTools-CSRF, Authorization",
    "Access-Control-Max-Age": "86400",
  };
  if (origin && ALLOWED_ORIGINS.has(origin)) {
    headers["Access-Control-Allow-Origin"] = origin;
    headers["Vary"] = "Origin";
  }
  return headers;
}

function json(status, value, origin, extra = {}) {
  return new Response(JSON.stringify(value), {
    status,
    headers: {
      ...corsHeaders(origin),
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      ...extra,
    },
  });
}

function text(status, value, origin, extra = {}) {
  return new Response(value, {
    status,
    headers: {
      ...corsHeaders(origin),
      "Content-Type": "text/plain; charset=utf-8",
      "Cache-Control": "no-store",
      ...extra,
    },
  });
}

function clientId(request, url) {
  const raw =
    request.headers.get("x-temptools-client") ||
    url.searchParams.get("client") ||
    "anonymous";
  const clean = raw.replace(/[^A-Za-z0-9_-]/g, "").slice(0, 64);
  return clean || "anonymous";
}

async function ensureSchema(db) {
  if (schemaReady) return;
  await db.batch([
    db.prepare(
      `CREATE TABLE IF NOT EXISTS mail_inboxes (
        id TEXT PRIMARY KEY,
        client_id TEXT NOT NULL,
        address TEXT NOT NULL,
        provider TEXT NOT NULL,
        password TEXT NOT NULL,
        token TEXT NOT NULL,
        remote_id TEXT NOT NULL,
        created_at REAL NOT NULL,
        expires_at REAL NOT NULL
      )`,
    ),
    db.prepare(
      "CREATE INDEX IF NOT EXISTS idx_mail_client ON mail_inboxes(client_id, expires_at)",
    ),
    db.prepare(
      `CREATE TABLE IF NOT EXISTS sms_numbers (
        id TEXT PRIMARY KEY,
        client_id TEXT NOT NULL,
        e164 TEXT NOT NULL,
        country TEXT NOT NULL,
        slug TEXT NOT NULL,
        provider TEXT NOT NULL,
        created_at REAL NOT NULL,
        expires_at REAL NOT NULL
      )`,
    ),
    db.prepare(
      "CREATE INDEX IF NOT EXISTS idx_sms_client ON sms_numbers(client_id, expires_at)",
    ),
    db.prepare(
      `CREATE TABLE IF NOT EXISTS web_settings (
        client_id TEXT PRIMARY KEY,
        appearance TEXT NOT NULL,
        notifications TEXT NOT NULL,
        updated_at REAL NOT NULL
      )`,
    ),
  ]);
  schemaReady = true;
}

function randomHex(bytes) {
  const values = new Uint8Array(bytes);
  crypto.getRandomValues(values);
  return Array.from(values, (value) => value.toString(16).padStart(2, "0")).join(
    "",
  );
}

function randomLocalPart() {
  const adjectives = [
    "quiet",
    "amber",
    "silver",
    "copper",
    "rapid",
    "velvet",
    "north",
    "hollow",
    "cobalt",
    "linen",
    "brisk",
    "mellow",
  ];
  const nouns = [
    "delta",
    "harbor",
    "falcon",
    "willow",
    "summit",
    "otter",
    "ember",
    "marlin",
    "canyon",
    "cedar",
    "pilot",
    "river",
  ];
  return `${adjectives[Math.floor(Math.random() * adjectives.length)]}-${
    nouns[Math.floor(Math.random() * nouns.length)]
  }-${randomHex(3)}`;
}

function parseTime(value) {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  const parsed = Date.parse(value || "");
  return Number.isFinite(parsed) ? parsed : now();
}

function sender(from) {
  if (!from) return "";
  if (typeof from === "string") return from;
  const address = from.address || "";
  return from.name ? `${from.name} <${address}>` : address;
}

function stripTags(input) {
  return String(input || "")
    .replace(/<script[\s\S]*?<\/script>/gi, " ")
    .replace(/<style[\s\S]*?<\/style>/gi, " ")
    .replace(/<[^>]*>/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&nbsp;/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function extractCode(value) {
  const message = String(value || "");
  const candidates = [
    /\b(?:code|otp|pin|passcode|verification)\D{0,32}(\d{4,8})\b/i,
    /\b(\d{4,8})\b[^.]{0,24}(?:code|otp|pin|verification)/i,
    /\b(\d{4,8})\b/,
  ];
  for (const pattern of candidates) {
    const match = message.match(pattern);
    if (match) return match[1] || match[0];
  }
  return "";
}

function extractLink(value) {
  const match = String(value || "").match(/https?:\/\/[^\s<>"']+/i);
  return match ? match[0].replace(/[),.;]+$/, "") : "";
}

function preview(value, limit = 100) {
  const clean = String(value || "").replace(/\s+/g, " ").trim();
  return clean.length > limit ? `${clean.slice(0, limit - 1)}…` : clean;
}

// --------------------------------------------------------------------- mail

async function mailTm(path, init = {}, token = "") {
  const headers = new Headers(init.headers || {});
  headers.set("Accept", "application/json");
  headers.set("User-Agent", SMS_AGENT);
  if (token) headers.set("Authorization", `Bearer ${token}`);
  if (init.body && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }
  const response = await fetch(`${MAIL_BASE}${path}`, {
    ...init,
    headers,
    signal: AbortSignal.timeout(20_000),
  });
  const raw = await response.text();
  let data = null;
  try {
    data = raw ? JSON.parse(raw) : null;
  } catch {
    data = null;
  }
  if (!response.ok) {
    const detail = data?.detail || data?.message || data?.error || raw.slice(0, 160);
    const error = new Error(`Mail.tm returned ${response.status}${detail ? `: ${detail}` : ""}`);
    error.status = response.status;
    throw error;
  }
  return data;
}

async function activeMailDomain() {
  const data = await mailTm("/domains?page=1");
  const domains = Array.isArray(data)
    ? data
    : data?.["hydra:member"] || data?.hydra?.member || [];
  const active = domains.find((item) => item.isActive !== false) || domains[0];
  if (!active?.domain) throw new Error("Mail.tm did not return an active domain");
  return active.domain;
}

async function guerrillaJson(path) {
  const response = await fetch(`https://api.guerrillamail.com/ajax.php?${path}`, {
    headers: { Accept: "application/json", "User-Agent": SMS_AGENT },
    signal: AbortSignal.timeout(20_000),
  });
  const raw = await response.text();
  let data = null;
  try {
    data = raw ? JSON.parse(raw) : null;
  } catch {
    data = null;
  }
  if (!response.ok) {
    throw new Error(`Guerrilla Mail returned ${response.status}`);
  }
  return data || {};
}

function guerrillaMessage(message) {
  const timestamp = Number(message.mail_timestamp || 0);
  return {
    id: String(message.mail_id || ""),
    from: message.mail_from || "",
    subject: message.mail_subject || "",
    intro: message.mail_excerpt || "",
    seen: Number(message.mail_read || 0) !== 0,
    createdAt: timestamp > 0 ? new Date(timestamp * 1000).toISOString() : message.mail_date || "",
  };
}

async function createGuerrillaInbox(db, client) {
  const created = await guerrillaJson("f=get_email_address&lang=en&site=guerrillamail.com");
  if (!created.email_addr || !created.sid_token) {
    throw new Error("Guerrilla Mail did not return an inbox");
  }
  const id = randomHex(20);
  const createdAt = now();
  const expiresAt = createdAt + MAIL_TTL_MS;
  await db
    .prepare(
      `INSERT INTO mail_inboxes
        (id, client_id, address, provider, password, token, remote_id, created_at, expires_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    )
    .bind(
      id,
      client,
      created.email_addr,
      "guerrilla",
      "",
      created.sid_token,
      "",
      createdAt,
      expiresAt,
    )
    .run();
  return { id, address: created.email_addr, provider: "guerrilla", created_at: createdAt, expires_at: expiresAt };
}

async function createMailInbox(db, client) {
  // One provider only: Guerrilla Mail is the only upstream this host can reach.
  return createGuerrillaInbox(db, client);
}

async function refreshMailToken(db, inbox) {
  const auth = await mailTm("/token", {
    method: "POST",
    body: JSON.stringify({ address: inbox.address, password: inbox.password }),
  });
  const token = auth?.token || "";
  inbox.token = token;
  await db
    .prepare("UPDATE mail_inboxes SET token = ? WHERE id = ?")
    .bind(token, inbox.id)
    .run();
  return token;
}

async function withMailToken(db, inbox, operation) {
  try {
    return await operation(inbox.token);
  } catch (error) {
    if (error.status !== 401) throw error;
    const token = await refreshMailToken(db, inbox);
    return operation(token);
  }
}

async function mailList(db, inbox) {
  if (inbox.provider === "guerrilla") {
    const data = await guerrillaJson(
      `f=get_email_list&offset=0&sid_token=${encodeURIComponent(inbox.token)}`,
    );
    return (data.list || [])
      .map(guerrillaMessage)
      .filter((message) => !(
        String(message.from).toLowerCase() === "no-reply@guerrillamail.com" &&
        String(message.subject).toLowerCase() === "welcome to guerrilla mail"
      ));
  }
  return withMailToken(db, inbox, async (token) => {
    const data = await mailTm("/messages?page=1", {}, token);
    return data?.["hydra:member"] || data?.hydra?.member || [];
  });
}

function mailBrief(inbox, message) {
  return {
    id: `${inbox.id}~${message.id}`,
    remote_id: String(message.id),
    sender: sender(message.from),
    subject: message.subject || "",
    preview: message.intro || "",
    received_at: parseTime(message.createdAt),
    seen: Boolean(message.seen),
  };
}

async function mailInboxRows(db, client) {
  const cutoff = now();
  await db
    .prepare("DELETE FROM mail_inboxes WHERE expires_at <= ?")
    .bind(cutoff)
    .run();
  const result = await db
    .prepare(
      "SELECT * FROM mail_inboxes WHERE client_id = ? AND expires_at > ? ORDER BY created_at DESC",
    )
    .bind(client, cutoff)
    .all();
  return result.results || [];
}

async function mailInboxesPayload(db, client) {
  const rows = await mailInboxRows(db, client);
  const inboxes = await Promise.all(
    rows.map(async (inbox) => {
      let messages = [];
      try {
        messages = await mailList(db, inbox);
      } catch {
        // A provider outage must not hide the inbox itself.
      }
      const unread = messages.filter((message) => !message.seen).length;
      const newest = messages[0];
      return {
        id: inbox.id,
        address: inbox.address,
        provider: inbox.provider,
        created_at: inbox.created_at,
        expires_at: inbox.expires_at,
        unread,
        last: newest
          ? {
              sender: sender(newest.from),
              subject: newest.subject || "",
              preview: newest.intro || "",
              received_at: parseTime(newest.createdAt),
            }
          : null,
      };
    }),
  );
  return { inboxes };
}

function mailMessagesPayload(inbox, messages) {
  const items = messages
    .map((message) => {
      const brief = mailBrief(inbox, message);
      return {
        id: brief.id,
        sender: brief.sender,
        subject: brief.subject,
        preview: brief.preview,
        received_at: brief.received_at,
        seen: brief.seen,
      };
    })
    .sort((a, b) => b.received_at - a.received_at);
  return { count: items.length, messages: items };
}

function findMailInbox(rows, id) {
  const [inboxId, remoteId] = String(id).split("~", 2);
  const inbox = rows.find((row) => row.id === inboxId);
  return { inbox, remoteId };
}

async function mailDetail(db, inbox, id) {
  const remoteId = String(id).split("~").slice(1).join("~");
  if (inbox.provider === "guerrilla") {
    const message = await guerrillaJson(
      `f=fetch_email&email_id=${encodeURIComponent(remoteId)}&sid_token=${encodeURIComponent(inbox.token)}`,
    );
    const body = String(message.mail_body || "");
    const isHtml = body.trimStart().startsWith("<");
    const plain = isHtml ? stripTags(body) : body;
    const timestamp = Number(message.mail_timestamp || 0);
    return {
      id,
      sender: message.mail_from || "",
      subject: message.mail_subject || "",
      text: plain,
      code: extractCode(`${plain} ${message.mail_subject || ""}`),
      link: extractLink(`${plain} ${body}`),
      received_at: timestamp > 0 ? timestamp * 1000 : now(),
      has_html: isHtml,
    };
  }
  const message = await withMailToken(db, inbox, async (token) =>
    mailTm(`/messages/${encodeURIComponent(remoteId)}`, {}, token),
  );
  const htmlValue = Array.isArray(message.html)
    ? message.html.join("\n")
    : message.html || "";
  const plain = message.text || stripTags(htmlValue);
  return {
    id,
    sender: sender(message.from),
    subject: message.subject || "",
    text: plain,
    code: extractCode(`${plain} ${message.subject || ""}`),
    link: extractLink(`${plain} ${htmlValue}`),
    received_at: parseTime(message.createdAt),
    has_html: Boolean(htmlValue.trim()),
  };
}

async function mailRoute(request, sub, url, env, origin, client) {
  const method = request.method;
  if (method === "GET" && sub === "/api/status") {
    const rows = await mailInboxRows(env.DB, client);
    return json(
      200,
      {
        mode: "web",
        providers: [
          {
            id: "guerrilla",
            label: "Guerrilla Mail",
            address_rule: "provider-assigned Guerrilla domain",
            inboxes: rows.filter((row) => row.provider === "guerrilla").length,
          },
        ],
        provider_rule: "Guerrilla Mail, fetched server-side",
      },
      origin,
    );
  }
  if (method === "GET" && sub === "/api/acceptance") {
    return json(200, { provider_health: [] }, origin);
  }
  if (method === "GET" && sub === "/api/inboxes") {
    return json(200, await mailInboxesPayload(env.DB, client), origin);
  }
  if (method === "POST" && sub === "/api/inboxes") {
    let parsed = {};
    try {
      parsed = await request.json();
    } catch {
      parsed = {};
    }
    // Older cached pages may still ask for Mail.tm; the hosted API has no such
    // provider, so any provider value resolves to Guerrilla Mail.
    const created = await createMailInbox(env.DB, client);
    return json(
      200,
      {
        ...created,
        requested_provider: parsed?.provider || created.provider,
        fallback: false,
      },
      origin,
    );
  }

  const inboxMatch = sub.match(/^\/api\/inboxes\/([^/]+)$/);
  if (inboxMatch && method === "DELETE") {
    const rows = await mailInboxRows(env.DB, client);
    const inbox = rows.find((row) => row.id === decodeURIComponent(inboxMatch[1]));
    if (!inbox) return json(404, { error: "Inbox not found" }, origin);
    try {
      if (inbox.provider === "guerrilla") {
        await guerrillaJson(`f=forget_me&sid_token=${encodeURIComponent(inbox.token)}`);
      } else {
        await withMailToken(env.DB, inbox, async (token) =>
          mailTm(`/accounts/${encodeURIComponent(inbox.remote_id)}`, { method: "DELETE" }, token),
        );
      }
    } catch {
      // The local row is still removed if the provider already forgot it.
    }
    await env.DB.prepare("DELETE FROM mail_inboxes WHERE id = ?").bind(inbox.id).run();
    return json(200, {}, origin);
  }

  const messagesMatch = sub.match(/^\/api\/inboxes\/([^/]+)\/messages$/);
  if (messagesMatch && method === "GET") {
    const rows = await mailInboxRows(env.DB, client);
    const inbox = rows.find((row) => row.id === decodeURIComponent(messagesMatch[1]));
    if (!inbox) return json(404, { error: "Inbox not found" }, origin);
    const messages = await mailList(env.DB, inbox);
    return json(200, mailMessagesPayload(inbox, messages), origin);
  }

  const htmlMatch = sub.match(/^\/api\/messages\/([^/]+)\/html$/);
  if (htmlMatch && method === "GET") {
    const rows = await mailInboxRows(env.DB, client);
    const { inbox } = findMailInbox(rows, decodeURIComponent(htmlMatch[1]));
    if (!inbox) return json(404, { error: "Message not found" }, origin);
    const remoteId = decodeURIComponent(htmlMatch[1]).split("~").slice(1).join("~");
    let html = "";
    if (inbox.provider === "guerrilla") {
      const message = await guerrillaJson(
        `f=fetch_email&email_id=${encodeURIComponent(remoteId)}&sid_token=${encodeURIComponent(inbox.token)}`,
      );
      html = String(message.mail_body || "");
    } else {
      const message = await withMailToken(env.DB, inbox, async (token) =>
        mailTm(`/messages/${encodeURIComponent(remoteId)}`, {}, token),
      );
      html = Array.isArray(message.html) ? message.html.join("\n") : message.html || "";
    }
    return new Response(html, {
      status: 200,
      headers: {
        ...corsHeaders(origin),
        "Content-Type": "text/html; charset=utf-8",
        "Cache-Control": "no-store",
        "X-Content-Type-Options": "nosniff",
        "Content-Security-Policy": "default-src 'none'; img-src https: data:; style-src 'unsafe-inline'",
      },
    });
  }

  const detailMatch = sub.match(/^\/api\/messages\/([^/]+)$/);
  if (detailMatch && method === "GET") {
    const rows = await mailInboxRows(env.DB, client);
    const { inbox } = findMailInbox(rows, decodeURIComponent(detailMatch[1]));
    if (!inbox) return json(404, { error: "Message not found" }, origin);
    return json(
      200,
      await mailDetail(env.DB, inbox, decodeURIComponent(detailMatch[1])),
      origin,
    );
  }

  return json(404, { error: "Mail route not found" }, origin);
}

// ---------------------------------------------------------------------- sms

function innerAfter(block, marker) {
  const at = block.indexOf(marker);
  if (at < 0) return "";
  const rest = block.slice(at);
  const open = rest.indexOf(">");
  if (open < 0) return "";
  const content = rest.slice(open + 1);
  const close = content.indexOf("</div>");
  return (close >= 0 ? content.slice(0, close) : content).trim();
}

function attrValue(block, prefix) {
  const at = block.indexOf(prefix);
  if (at < 0) return "";
  const rest = block.slice(at + prefix.length);
  const end = rest.indexOf('"');
  return end >= 0 ? rest.slice(0, end) : "";
}

function smsCountries(html) {
  const out = [];
  const needle = "/temporary-numbers/";
  let from = 0;
  while (true) {
    const at = html.indexOf(needle, from);
    if (at < 0) break;
    const start = at + needle.length;
    const rest = html.slice(start);
    const end = rest.search(/["'<\s]/);
    const path = end >= 0 ? rest.slice(0, end) : rest;
    const parts = path.split("/").filter(Boolean);
    if (parts.length === 1 && /^[a-z-]+$/.test(parts[0])) out.push(parts[0]);
    from = start;
  }
  return [...new Set(out)].sort();
}

function smsNumbers(html) {
  const out = [];
  const needle = "/temporary-numbers/";
  let from = 0;
  while (true) {
    const at = html.indexOf(needle, from);
    if (at < 0) break;
    const start = at + needle.length;
    const rest = html.slice(start);
    const end = rest.search(/["'<\s]/);
    const path = end >= 0 ? rest.slice(0, end) : rest;
    const parts = path.split("/").filter(Boolean);
    if (parts.length === 2 && /^[a-z-]+$/.test(parts[0]) && /^\d{7,15}$/.test(parts[1])) {
      out.push([parts[0], parts[1]]);
    }
    from = start;
  }
  const deduped = new Map();
  for (const [country, digits] of out) deduped.set(`${country}/${digits}`, [country, digits]);
  return [...deduped.values()].sort((a, b) => a[1].localeCompare(b[1]));
}

function smsMessages(html) {
  const out = [];
  let from = 0;
  while (true) {
    const at = html.indexOf('class="sms-item"', from);
    if (at < 0) break;
    const rest = html.slice(at);
    const next = rest.indexOf('class="sms-item"', 10);
    const block = next >= 0 ? rest.slice(0, next) : rest;
    const sender = innerAfter(block, 'class="left"');
    const when = innerAfter(block, 'class="right"');
    const body = innerAfter(block, 'class="center"');
    const seconds = Number(attrValue(block, 'data-time="') || 0);
    const cleanBody = stripTags(body);
    if (cleanBody) {
      out.push({
        sender: stripTags(sender),
        body: cleanBody,
        received_at: seconds > 0 ? seconds * 1000 : 0,
        when,
      });
    }
    from = at + Math.max(block.length, 1);
  }
  return out;
}

async function smsFetch(url) {
  const response = await fetch(url, {
    headers: { "User-Agent": SMS_AGENT, Accept: "text/html,application/xhtml+xml" },
    signal: AbortSignal.timeout(20_000),
  });
  if (!response.ok) throw new Error(`receive-sms.io returned ${response.status}`);
  return response.text();
}

function smsMarkSuccess(retries = 0) {
  smsHealth = {
    ...smsHealth,
    status: "ok",
    consecutive_failures: 0,
    last_error: "",
    last_retry_count: retries,
    last_source: "live",
    last_attempt_at: now(),
    last_success_at: now(),
  };
}

function smsMarkFailure(error, usingCache = false) {
  smsHealth = {
    ...smsHealth,
    status: usingCache ? "degraded" : "error",
    consecutive_failures: (smsHealth.consecutive_failures || 0) + 1,
    last_error: String(error),
    last_source: usingCache ? "cache" : "live",
    last_attempt_at: now(),
    last_failure_at: now(),
  };
}

async function smsCountriesList(fresh = false) {
  if (!fresh && countryCache && now() - countryCache.at < 10 * 60 * 1000) {
    return countryCache.list;
  }
  try {
    const html = await smsFetch(`${SMS_BASE}/`);
    const list = smsCountries(html);
    if (!list.length) throw new Error("the source listed no countries");
    countryCache = { at: now(), list };
    smsMarkSuccess();
    return list;
  } catch (error) {
    if (countryCache) {
      smsMarkFailure(error, true);
      return countryCache.list;
    }
    smsMarkFailure(error);
    throw error;
  }
}

async function smsNumbersForCountry(slug) {
  const html = await smsFetch(`${SMS_BASE}/temporary-numbers/${encodeURIComponent(slug)}/`);
  const numbers = smsNumbers(html)
    .filter(([country]) => country === slug)
    .map(([, digits]) => digits);
  if (!numbers.length) throw new Error("no public numbers are listed for this country right now");
  smsMarkSuccess();
  return numbers;
}

async function smsMessagesFor(number, fresh = false) {
  const url = `${SMS_BASE}/temporary-numbers/${encodeURIComponent(number.slug)}/${number.e164.replace(/\D/g, "")}/`;
  const html = await smsFetch(url);
  const messages = smsMessages(html);
  smsMarkSuccess();
  return messages;
}

function smsMessageId(number, message, index) {
  return `${number.id}-${Math.round(message.received_at || now())}-${index}`;
}

async function smsRows(db, client) {
  const cutoff = now();
  await db.prepare("DELETE FROM sms_numbers WHERE expires_at <= ?").bind(cutoff).run();
  const result = await db
    .prepare(
      "SELECT * FROM sms_numbers WHERE client_id = ? AND expires_at > ? ORDER BY created_at DESC",
    )
    .bind(client, cutoff)
    .all();
  return result.results || [];
}

async function smsRoute(request, sub, url, env, origin, client) {
  const method = request.method;
  if (method === "GET" && sub === "/api/status") {
    const rows = await smsRows(env.DB, client);
    return json(
      200,
      {
        mode: "live",
        provider: "receive-sms.io",
        numbers: rows.length,
        source: "public shared inboxes",
        source_health: smsHealth,
        provider_rule: "live public page, then last-good cache",
        fallback_order: ["live public page", "last good cache"],
        webhook: { twilio: false, twilio_url_override: false, telnyx: false, generic: false },
      },
      origin,
    );
  }
  if (method === "GET" && sub === "/api/health") {
    return json(
      200,
      { source: smsHealth, provider_rule: "live public page, then last-good cache" },
      origin,
    );
  }
  if (method === "GET" && sub === "/api/countries") {
    try {
      const countries = await smsCountriesList();
      return json(200, { countries, source_health: smsHealth }, origin);
    } catch (error) {
      return json(502, { error: String(error), source_health: smsHealth }, origin);
    }
  }
  if (method === "POST" && sub === "/api/source/check") {
    try {
      const countries = await smsCountriesList(true);
      return json(
        200,
        { ok: true, country_count: countries.length, countries, source_health: smsHealth },
        origin,
      );
    } catch (error) {
      return json(502, { error: String(error), source_health: smsHealth }, origin);
    }
  }
  const countryNumbers = sub.match(/^\/api\/countries\/([^/]+)\/numbers$/);
  if (method === "GET" && countryNumbers) {
    const slug = decodeURIComponent(countryNumbers[1]);
    try {
      if (!/^[a-z-]+$/.test(slug)) return json(422, { error: "invalid country slug" }, origin);
      const numbers = await smsNumbersForCountry(slug);
      return json(
        200,
        { country: slug, numbers, source: "public shared inboxes", source_health: smsHealth },
        origin,
      );
    } catch (error) {
      return json(502, { error: String(error), source_health: smsHealth }, origin);
    }
  }
  if (method === "GET" && sub === "/api/numbers") {
    const rows = await smsRows(env.DB, client);
    return json(
      200,
      {
        numbers: rows.map((row) => ({
          id: row.id,
          e164: row.e164,
          country: row.country,
          provider: row.provider,
          created_at: row.created_at,
          expires_at: row.expires_at,
        })),
      },
      origin,
    );
  }
  if (method === "POST" && sub === "/api/numbers") {
    let parsed = {};
    try {
      parsed = await request.json();
    } catch {
      parsed = {};
    }
    const slug = String(parsed.country || "").trim().toLowerCase();
    const rawNumber = String(parsed.number || "").replace(/[^\d+]/g, "");
    const digits = rawNumber.replace(/\D/g, "");
    if (!/^[a-z-]+$/.test(slug) || digits.length < 7 || digits.length > 15) {
      return json(422, { error: "choose a valid country and public number" }, origin);
    }
    const e164 = `+${digits}`;
    const rows = await smsRows(env.DB, client);
    const existing = rows.find((row) => row.e164 === e164);
    if (existing) return json(200, { id: existing.id, e164: existing.e164 }, origin);
    const id = randomHex(20);
    const createdAt = now();
    await env.DB.prepare(
      `INSERT INTO sms_numbers
        (id, client_id, e164, country, slug, provider, created_at, expires_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
    )
      .bind(id, client, e164, slug, slug, "receive-sms.io", createdAt, createdAt + SMS_TTL_MS)
      .run();
    return json(200, { id, e164 }, origin);
  }

  const deleteMatch = sub.match(/^\/api\/numbers\/([^/]+)$/);
  if (method === "DELETE" && deleteMatch) {
    await env.DB.prepare("DELETE FROM sms_numbers WHERE id = ? AND client_id = ?")
      .bind(decodeURIComponent(deleteMatch[1]), client)
      .run();
    return json(200, {}, origin);
  }

  const messagesMatch = sub.match(/^\/api\/numbers\/([^/]+)\/messages$/);
  if (method === "GET" && messagesMatch) {
    const rows = await smsRows(env.DB, client);
    const number = rows.find((row) => row.id === decodeURIComponent(messagesMatch[1]));
    if (!number) return json(404, { error: "Number not found" }, origin);
    let messages = [];
    try {
      messages = await smsMessagesFor(number, url.searchParams.get("fresh") === "1");
    } catch (error) {
      return json(502, { error: String(error), source_health: smsHealth }, origin);
    }
    const items = messages
      .map((message, index) => {
        const id = smsMessageId(number, message, index);
        return {
          id,
          sender: message.sender,
          body: message.body,
          code: extractCode(message.body),
          preview: preview(message.body, 90),
          received_at: message.received_at || now(),
          seen: false,
        };
      })
      .sort((a, b) => b.received_at - a.received_at);
    return json(
      200,
      {
        number: { e164: number.e164, country: number.country },
        count: items.length,
        messages: items,
        source_health: smsHealth,
      },
      origin,
    );
  }

  const detailMatch = sub.match(/^\/api\/messages\/([^/]+)$/);
  if (method === "GET" && detailMatch) {
    const rawId = decodeURIComponent(detailMatch[1]);
    const rows = await smsRows(env.DB, client);
    const number = rows.find((row) => rawId.startsWith(`${row.id}-`));
    if (!number) return json(404, { error: "Message not found" }, origin);
    try {
      const messages = await smsMessagesFor(number);
      const found = messages.find((message, index) => smsMessageId(number, message, index) === rawId);
      if (!found) return json(404, { error: "Message not found" }, origin);
      return json(
        200,
        {
          id: rawId,
          sender: found.sender,
          body: found.body,
          code: extractCode(found.body),
          received_at: found.received_at || now(),
          number: number.e164,
        },
        origin,
      );
    } catch (error) {
      return json(502, { error: String(error), source_health: smsHealth }, origin);
    }
  }

  return json(404, { error: "SMS route not found" }, origin);
}

// ----------------------------------------------------------------- settings

const RELEASE_NOTES = [
  {
    tool: "Mail",
    version: "0.3.0",
    summary: "Disposable addresses, live inbox refresh and code extraction.",
    changelog: [
      { version: "0.3.0", change: "Hosted web build using Guerrilla Mail through the serverless API." },
    ],
  },
  {
    tool: "SMS",
    version: "0.3.0",
    summary: "Public temporary numbers and received texts.",
    changelog: [
      { version: "0.3.0", change: "Hosted web build using receive-sms.io through the serverless API." },
    ],
  },
  {
    tool: "OTP",
    version: "0.3.0",
    summary: "Offline code extraction and a browser-local TOTP vault.",
    changelog: [
      { version: "0.3.0", change: "Authenticator secrets stay in this browser's localStorage." },
    ],
  },
];

function defaultSettings() {
  return {
    appearance: "dark",
    notifications: { enabled: true, mail: true, sms: true },
    owner: "drifter1987",
    version: VERSION,
    paths: [],
    release_notes: RELEASE_NOTES,
  };
}

function settingsPayload(row) {
  const base = defaultSettings();
  if (!row) return base;
  let notifications = base.notifications;
  try {
    notifications = JSON.parse(row.notifications);
  } catch {
    // Keep the defaults when an old row is malformed.
  }
  return {
    ...base,
    appearance: row.appearance === "light" ? "light" : "dark",
    notifications,
  };
}

async function settingsRoute(request, env, origin, client) {
  const row = await env.DB.prepare("SELECT * FROM web_settings WHERE client_id = ?")
    .bind(client)
    .first();
  if (request.method === "GET") {
    return json(200, settingsPayload(row), origin);
  }
  if (request.method !== "POST") return json(405, { error: "Method not allowed" }, origin);
  let parsed = {};
  try {
    parsed = await request.json();
  } catch {
    return json(422, { error: "Expected a JSON body" }, origin);
  }
  const current = settingsPayload(row);
  if (parsed.appearance !== undefined) {
    if (parsed.appearance !== "dark" && parsed.appearance !== "light") {
      return json(422, { error: "appearance must be dark or light" }, origin);
    }
    current.appearance = parsed.appearance;
  }
  if (parsed.notifications !== undefined) {
    const next = parsed.notifications || {};
    if (
      typeof next.enabled !== "boolean" ||
      typeof next.mail !== "boolean" ||
      typeof next.sms !== "boolean"
    ) {
      return json(422, { error: "notifications must contain enabled, mail and sms booleans" }, origin);
    }
    current.notifications = { enabled: next.enabled, mail: next.mail, sms: next.sms };
  }
  await env.DB.prepare(
    `INSERT INTO web_settings (client_id, appearance, notifications, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(client_id) DO UPDATE SET
       appearance = excluded.appearance,
       notifications = excluded.notifications,
       updated_at = excluded.updated_at`,
  )
    .bind(client, current.appearance, JSON.stringify(current.notifications), now())
    .run();
  return json(200, current, origin);
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const origin = request.headers.get("origin") || "";
    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: corsHeaders(origin) });
    }
    if (!env.DB) {
      return json(500, { error: "The API database is not configured" }, origin);
    }
    await ensureSchema(env.DB);
    const client = clientId(request, url);
    const sub = url.pathname.replace(/\/+$/, "") || "/";

    try {
      if (sub === "/api/status") {
        const mail = await env.DB.prepare("SELECT COUNT(*) AS count FROM mail_inboxes WHERE client_id = ? AND expires_at > ?")
          .bind(client, now())
          .first();
        const sms = await env.DB.prepare("SELECT COUNT(*) AS count FROM sms_numbers WHERE client_id = ? AND expires_at > ?")
          .bind(client, now())
          .first();
        return json(
          200,
          {
            mode: "web",
            version: VERSION,
            mail: { provider: "guerrilla", inboxes: mail?.count || 0 },
            sms: { provider: "receive-sms.io", numbers: sms?.count || 0 },
            otp: "browser-local",
          },
          origin,
        );
      }
      if (sub === "/api/settings") {
        return await settingsRoute(request, env, origin, client);
      }
      if (sub === "/api/notifications/test" && request.method === "POST") {
        return json(200, { ok: true }, origin);
      }
      if (sub === "/api/clear-data" && request.method === "POST") {
        let parsed = {};
        try {
          parsed = await request.json();
        } catch {
          parsed = {};
        }
        if (parsed.confirm !== "CLEAR") {
          return json(422, { error: 'Send {"confirm":"CLEAR"} to delete hosted data' }, origin);
        }
        await env.DB.batch([
          env.DB.prepare("DELETE FROM mail_inboxes WHERE client_id = ?").bind(client),
          env.DB.prepare("DELETE FROM sms_numbers WHERE client_id = ?").bind(client),
          env.DB.prepare("DELETE FROM web_settings WHERE client_id = ?").bind(client),
        ]);
        return json(200, { ok: true, restart_required: false }, origin);
      }
      if (sub.startsWith("/api/mail/")) {
        return await mailRoute(request, sub.slice("/api/mail".length), url, env, origin, client);
      }
      if (sub.startsWith("/api/sms/")) {
        return await smsRoute(request, sub.slice("/api/sms".length), url, env, origin, client);
      }
      if (sub.startsWith("/api/otp")) {
        return json(
          501,
          { error: "OTP is local to the browser in the hosted build; no secret is sent to this API." },
          origin,
        );
      }
      return json(404, { error: "Not found" }, origin);
    } catch (error) {
      return json(
        502,
        { error: error instanceof Error ? error.message : String(error) },
        origin,
      );
    }
  },
};
