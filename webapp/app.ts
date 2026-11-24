/**
 * Browser client for GMO TempTools. It is served by the desktop process on
 * 127.0.0.1:8780 and uses the same APIs and OTP vault as the native tabs.
 */

(() => {
  type Screen = "mail" | "sms" | "otp" | "settings";

  interface ApiError { error?: string; code?: string; retry_after?: number; limit?: number }
  interface Provider { id: string; label: string; address_rule?: string; inboxes?: number }
  interface ProviderHealth {
    provider: string;
    list_failures: number;
    create_failures: number;
    last_error: string;
    last_create_error: string;
    last_success_at: number;
    last_attempt_at: number;
    error_is_current: boolean;
  }
  interface SourceHealth {
    provider: string;
    status: string;
    consecutive_failures: number;
    last_error: string;
    last_retry_count: number;
    last_source: string;
    last_attempt_at?: number;
    last_success_at?: number;
    last_failure_at?: number;
  }
  interface HealthProvider {
    provider: string;
    label: string;
    kind: string;
    status: string;
    consecutive_failures: number;
    last_error: string;
    last_success_at: number;
    last_attempt_at: number;
  }
  interface MailLast { sender: string; subject: string; preview?: string; received_at: number }
  interface MailInbox {
    id: string;
    address: string;
    provider: string;
    created_at: number;
    expires_at: number;
    unread: number;
    last?: MailLast | null;
  }
  interface MailMessage {
    id: string;
    sender: string;
    subject: string;
    preview?: string;
    received_at: number;
    seen: boolean;
  }
  interface MailDetail {
    id: string;
    sender: string;
    subject: string;
    text?: string;
    code?: string;
    link?: string;
    received_at: number;
    has_html: boolean;
  }
  interface SmsNumber {
    id: string;
    e164: string;
    country: string;
    provider: string;
    created_at: number;
    expires_at: number;
  }
  interface SmsMessage {
    id: string;
    sender: string;
    preview?: string;
    body?: string;
    received_at: number;
    seen: boolean;
  }
  interface SmsDetail {
    id: string;
    sender: string;
    body?: string;
    code?: string;
    received_at: number;
  }
  interface WebhookStatus {
    twilio: boolean;
    twilio_url_override: boolean;
    telnyx: boolean;
    generic: boolean;
  }
  interface ServiceStatus {
    providers?: Provider[];
    source_health?: SourceHealth;
    provider_rule?: string;
    webhook?: WebhookStatus;
    webhook_configured?: boolean;
    numbers?: number;
    limits?: { inboxes?: number; numbers?: number };
  }
  interface OtpSecret {
    index: number;
    label: string;
    masked: string;
    code: string;
    digits: number;
    period: number;
    algorithm: string;
    remaining: number;
  }
  interface FoundCodes { codes: string[]; links: string[] }
  interface SettingsPath { label: string; path: string }
  interface ChangelogEntry { version: string; change: string }
  interface ToolRelease {
    tool: string;
    version: string;
    summary: string;
    changelog: ChangelogEntry[];
  }
  interface NotificationSettings {
    enabled: boolean;
    mail: boolean;
    sms: boolean;
  }
  interface AppSettings {
    appearance: "dark" | "light";
    notifications: NotificationSettings;
    owner: string;
    version: string;
    paths: SettingsPath[];
    release_notes: ToolRelease[];
  }
  interface LocalOtpSecret {
    label: string;
    secret: string;
    digits: number;
    period: number;
    algorithm: "SHA1" | "SHA256" | "SHA512";
  }

  const API_BASE = ((
    window as unknown as { TEMPTOOLS_API_BASE?: string }
  ).TEMPTOOLS_API_BASE ?? "").replace(/\/+$/, "");
  const WEB = API_BASE.length > 0;
  const CLIENT_ID_KEY = "gmo-temptools.client.v1";
  const LOCAL_OTP_KEY = "gmo-temptools.otp.v1";
  const LOCAL_OTP_BACKUP_KEY = "gmo-temptools.otp.backup-at.v1";

  const app = GmoUI.$<HTMLElement>("#app") as HTMLElement;

  const { esc, copy, readJson, toast } = GmoUI;
  const screens: Screen[] = ["mail", "sms", "otp", "settings"];
  let current: Screen = "mail";
  let renderToken = 0;
  let timers: number[] = [];

  const mail = {
    providers: [] as Provider[],
    health: [] as ProviderHealth[],
    healthUnavailable: false,
    inboxes: [] as MailInbox[],
    unread: {} as Record<string, number>,
    selected: null as string | null,
    messages: [] as MailMessage[],
    detail: null as MailDetail | null,
    provider: WEB ? "guerrilla" : "mail.tm",
    limit: 0,
  };
  const sms = {
    numbers: [] as SmsNumber[],
    selected: null as string | null,
    messages: [] as SmsMessage[],
    detail: null as SmsDetail | null,
    status: null as ServiceStatus | null,
    countries: [] as string[],
    country: "",
    available: [] as string[],
    chosen: "",
    limit: 0,
  };
  const otp = {
    tab: "extract" as "extract" | "generate",
    paste: "",
    found: null as FoundCodes | null,
    uri: "",
    label: "",
    secrets: [] as OtpSecret[],
  };
  const settingsState = {
    appearance: "dark" as "dark" | "light",
    notifications: { enabled: true, mail: true, sms: true } as NotificationSettings,
    owner: "",
    version: "",
    paths: [] as SettingsPath[],
    notes: [] as ToolRelease[],
    health: [] as HealthProvider[],
    status: "",
  };

  if (WEB) {
    const mode = GmoUI.$<HTMLElement>("#mode-status");
    if (mode) mode.innerHTML = '<span class="pulse"></span> hosted web';
    const footer = GmoUI.$<HTMLElement>("#footer-mode");
    if (footer) footer.textContent = "mail + SMS via API · otp in this browser";
    if ("serviceWorker" in navigator) {
      void navigator.serviceWorker.register("./sw.js").catch(() => {
        // Without a worker the app still works online; offline is a bonus.
      });
    }
    window.addEventListener("online", paintConnectivity);
    window.addEventListener("offline", paintConnectivity);
    paintConnectivity();
  }

  function isScreen(value: string): value is Screen {
    return screens.indexOf(value as Screen) >= 0;
  }

  function stopTimers(): void {
    timers.forEach((timer) => window.clearInterval(timer));
    timers = [];
  }

  function addTimer(callback: () => void, milliseconds: number): void {
    timers.push(window.setInterval(callback, milliseconds));
  }

  function paintConnectivity(): void {
    if (!WEB) return;
    let banner = document.getElementById("offline-banner");
    if (!banner) {
      banner = document.createElement("div");
      banner.id = "offline-banner";
      banner.className = "offline-banner";
      banner.setAttribute("role", "status");
      banner.textContent =
        "Offline — Mail and SMS need a connection. OTP and the saved vault still work.";
      document.body.appendChild(banner);
    }
    banner.hidden = navigator.onLine;
  }

  function clientId(): string {
    try {
      let id = localStorage.getItem(CLIENT_ID_KEY) ?? "";
      if (!id) {
        id = typeof crypto.randomUUID === "function"
          ? crypto.randomUUID()
          : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
        localStorage.setItem(CLIENT_ID_KEY, id);
      }
      return id;
    } catch {
      return "anonymous";
    }
  }

  function loadLocalSecrets(): LocalOtpSecret[] {
    try {
      const raw = JSON.parse(localStorage.getItem(LOCAL_OTP_KEY) ?? "[]") as LocalOtpSecret[];
      return Array.isArray(raw)
        ? raw.filter((item) => Boolean(item && item.secret && item.label))
        : [];
    } catch {
      return [];
    }
  }

  function saveLocalSecrets(secrets: LocalOtpSecret[]): void {
    localStorage.setItem(LOCAL_OTP_KEY, JSON.stringify(secrets));
  }

  function base32Bytes(value: string): Uint8Array {
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    const clean = value.toUpperCase().replace(/[^A-Z2-7]/g, "");
    let bits = 0;
    let accumulator = 0;
    const out: number[] = [];
    for (const char of clean) {
      const digit = alphabet.indexOf(char);
      if (digit < 0) continue;
      accumulator = (accumulator << 5) | digit;
      bits += 5;
      if (bits >= 8) {
        bits -= 8;
        out.push((accumulator >>> bits) & 0xff);
      }
    }
    return new Uint8Array(out);
  }

  function parseOtpSecret(input: string, label: string): LocalOtpSecret | null {
    const raw = input.trim();
    let secret = raw;
    let resolvedLabel = label.trim() || "Authenticator";
    let digits = 6;
    let period = 30;
    let algorithm: LocalOtpSecret["algorithm"] = "SHA1";
    if (raw.toLowerCase().startsWith("otpauth://")) {
      try {
        const parsed = new URL(raw);
        if (parsed.hostname.toLowerCase() !== "totp") return null;
        secret = parsed.searchParams.get("secret") ?? "";
        digits = Number(parsed.searchParams.get("digits") ?? "6");
        period = Number(parsed.searchParams.get("period") ?? "30");
        const algorithmName = (parsed.searchParams.get("algorithm") ?? "SHA1").toUpperCase();
        if (algorithmName === "SHA256" || algorithmName === "SHA512") algorithm = algorithmName;
        resolvedLabel = label.trim() || decodeURIComponent(parsed.pathname.replace(/^\/+/, "")) || "Authenticator";
      } catch {
        return null;
      }
    }
    secret = secret.replace(/[\s-]/g, "").toUpperCase();
    if (!/^[A-Z2-7]+=*$/.test(secret) || secret.replace(/=/g, "").length < 8) return null;
    if (digits !== 6 && digits !== 8) return null;
    if (!Number.isFinite(period) || period < 15 || period > 120) return null;
    return { label: resolvedLabel, secret, digits, period, algorithm };
  }

  function maskSecret(secret: string): string {
    return `${secret.slice(0, 4)}…${secret.slice(-4)}`;
  }

  async function totpCode(secret: LocalOtpSecret): Promise<string> {
    const rawSecret = base32Bytes(secret.secret);
    const key = await crypto.subtle.importKey(
      "raw",
      rawSecret.buffer as ArrayBuffer,
      { name: "HMAC", hash: `SHA-${secret.algorithm === "SHA512" ? "512" : secret.algorithm === "SHA256" ? "256" : "1"}` },
      false,
      ["sign"],
    );
    const counter = new Uint8Array(8);
    let value = Math.floor(Date.now() / 1000 / secret.period);
    for (let index = 7; index >= 0; index -= 1) {
      counter[index] = value & 0xff;
      value = Math.floor(value / 256);
    }
    const signature = new Uint8Array(await crypto.subtle.sign("HMAC", key, counter));
    const offset = signature[signature.length - 1] & 0x0f;
    const binary =
      ((signature[offset] & 0x7f) << 24) |
      ((signature[offset + 1] & 0xff) << 16) |
      ((signature[offset + 2] & 0xff) << 8) |
      (signature[offset + 3] & 0xff);
    return String(binary % 10 ** secret.digits).padStart(secret.digits, "0");
  }

  async function localOtpPayload(): Promise<{ secrets: OtpSecret[] }> {
    const now = Date.now() / 1000;
    const secrets = loadLocalSecrets();
    return {
      secrets: await Promise.all(secrets.map(async (secret, index) => ({
        index,
        label: secret.label,
        masked: maskSecret(secret.secret),
        code: await totpCode(secret),
        digits: secret.digits,
        period: secret.period,
        algorithm: secret.algorithm,
        remaining: Math.max(0, Math.ceil(secret.period - (now % secret.period))),
      }))),
    };
  }

  function extractFromText(input: string): FoundCodes {
    const codes: string[] = [];
    const patterns = [
      /(?:code|otp|pin|passcode|verification)[^0-9]{0,24}(\d{4,8})/gi,
      /(\d{4,8})[^0-9]{0,24}(?:code|otp|pin|verification)/gi,
      /\b(\d{4,8})\b/g,
    ];
    for (const pattern of patterns) {
      for (const match of input.matchAll(pattern)) {
        const code = match[1];
        if (code && codes.indexOf(code) < 0) codes.push(code);
        if (codes.length >= 6) break;
      }
      if (codes.length >= 6) break;
    }
    const links = Array.from(input.matchAll(/https?:\/\/[^\s<>"']+/gi), (match) =>
      match[0].replace(/[),.;]+$/, ""),
    ).slice(0, 6);
    return { codes, links };
  }

  async function localOtpRequest<T>(url: string, init?: RequestInit): Promise<T> {
    const method = (init?.method ?? "GET").toUpperCase();
    const path = url.split("?")[0];
    if (method === "GET" && path === "/api/otp") {
      return (await localOtpPayload()) as T;
    }
    if (method === "POST" && path === "/api/otp/extract") {
      const body = JSON.parse(String(init?.body ?? "{}")) as { text?: string };
      return extractFromText(body.text ?? "") as T;
    }
    if (method === "POST" && path === "/api/otp") {
      const body = JSON.parse(String(init?.body ?? "{}")) as { input?: string; label?: string };
      const parsed = parseOtpSecret(body.input ?? "", body.label ?? "");
      if (!parsed) throw new Error("That is not an otpauth:// URI or a base32 secret (8+ characters).");
      const secrets = loadLocalSecrets();
      const exists = secrets.some((item) => item.secret === parsed.secret && item.label === parsed.label);
      if (!exists) {
        secrets.push(parsed);
        saveLocalSecrets(secrets);
      }
      return { ok: true, added: !exists } as T;
    }
    if (method === "DELETE" && path.startsWith("/api/otp/")) {
      const index = Number(path.slice("/api/otp/".length));
      const secrets = loadLocalSecrets();
      if (!Number.isInteger(index) || index < 0 || index >= secrets.length) {
        throw new Error("Secret not found");
      }
      secrets.splice(index, 1);
      saveLocalSecrets(secrets);
      return { ok: true } as T;
    }
    throw new Error("Unsupported local OTP request");
  }

  async function api<T>(url: string, init?: RequestInit): Promise<T> {
    const method = (init?.method ?? "GET").toUpperCase();
    if (WEB && url.split("?")[0].startsWith("/api/otp")) {
      return localOtpRequest<T>(url, init);
    }
    const headers = new Headers(init?.headers);
    if (method !== "GET" && method !== "HEAD") {
      headers.set("X-TempTools-CSRF", "1");
    }
    if (WEB) headers.set("X-TempTools-Client", clientId());
    const response = await fetch(`${API_BASE}${url}`, {
      cache: "no-store",
      credentials: WEB ? "omit" : "same-origin",
      ...(init ?? {}),
      headers,
    });
    const data = await readJson<T & ApiError>(response);
    if (!response.ok) {
      const error = new Error(data?.error ?? `Request failed (${response.status})`) as Error & { code?: string; retry_after?: number };
      error.code = data?.code;
      error.retry_after = data?.retry_after;
      throw error;
    }
    return data as T;
  }

  function fail(error: unknown): void {
    toast(error instanceof Error ? error.message : String(error), 3500);
  }

  function ttl(expires: number): string {
    return GmoUI.ttl(expires - Date.now());
  }

  function updateTtls(): void {
    GmoUI.updateTtls((remaining) => GmoUI.ttl(remaining));
  }

  function ttlSpan(expires: number): string {
    return `<span data-ttl data-expires="${expires}">${esc(ttl(expires))}</span>`;
  }

  // The hosted API caps how many temporary resources one browser may hold. Show
  // that cap with the time until the oldest slot frees, instead of letting a
  // raw 429 be the first hint.
  function capState(items: { expires_at: number }[], limit: number): { full: boolean; html: string } {
    if (!WEB || limit <= 0 || items.length < limit) return { full: false, html: "" };
    const soonest = Math.min(...items.map((item) => item.expires_at));
    return {
      full: true,
      html: `${items.length} of ${limit} in use — oldest slot: ${ttlSpan(soonest)}. Delete one to free a slot now.`,
    };
  }

  function capMessage(items: { expires_at: number }[], limit: number, noun: string): string {
    if (limit <= 0) return `This browser has reached its ${noun} limit. Delete one to make room.`;
    const soonest = Math.min(...items.map((item) => item.expires_at));
    return `All ${limit} ${noun} are in use. Oldest slot: ${ttl(soonest)}. Delete one to make room.`;
  }

  function show(screen: Screen): void {
    current = screen;
    renderToken += 1;
    stopTimers();
    document.querySelectorAll<HTMLButtonElement>("#main-tabs .main-tab").forEach((button) => {
      button.classList.toggle("on", button.dataset.screen === screen);
    });
    history.replaceState(null, "", `#${screen}`);
    app.innerHTML = `<p class="muted pad">Loading…</p>`;
    if (screen === "mail") void renderMail();
    if (screen === "sms") void renderSms();
    if (screen === "otp") void renderOtp();
    if (screen === "settings") renderSettings();
  }

  // ---------------------------------------------------------------- mail

  function mailShell(): string {
    return `
      <section class="intro">
        <div>
          <h1>Mail</h1>
          <p class="muted">Throwaway addresses and verification codes, ${WEB ? "kept in this browser's hosted session." : "kept locally."}</p>
        </div>
        <div class="intro-actions">
          <span class="chip live">live mail</span>
          <select class="select" id="mail-provider" aria-label="Mail provider"></select>
          <button class="btn primary" id="mail-new" data-action="mail-new">New inbox</button>
        </div>
      </section>
      <p class="note">Each inbox is fetched from the provider you chose when creating it and kept ${WEB ? "by the hosted API" : "locally"} for one hour.</p>
      <p class="muted small" id="mail-provider-note"></p>
      <p class="note" id="mail-cap" hidden></p>
      <p class="muted small" id="mail-health"></p>
      <div class="split">
        <div class="split-list" id="mail-list"><div class="pad muted">No inboxes loaded.</div></div>
        <div class="split-main" id="mail-thread">
          <div class="empty-reading"><p class="muted">Select a message to read it.</p></div>
        </div>
      </div>
      <details class="guide">
        <summary>How it works</summary>
        <ol>
          <li>Create an inbox and paste the address into a signup form.</li>
          <li>The verification mail arrives here; the code is pulled out automatically.</li>
          <li>Inboxes and their mail are deleted an hour after creation.</li>
          ${WEB
            ? "<li>Some senders block disposable domains. The hosted API offers Guerrilla Mail, Best-TempMail and TempMail.ai, and falls back automatically if one is down; the desktop app adds Mail.tm.</li>"
            : "<li>Some senders block disposable domains; switching provider is the practical workaround.</li>"}
        </ol>
        <p>No account is needed here. The message list is cached for a few seconds to stay inside provider free-tier limits.</p>
      </details>`;
  }

  function renderMail(): void {
    const token = ++renderToken;
    app.innerHTML = mailShell();
    void loadMail(token);
    addTimer(() => {
      if (current === "mail" && document.visibilityState === "visible") {
        void loadMailMessages();
      }
    }, 4_000);
    addTimer(() => {
      if (current === "mail" && document.visibilityState === "visible") {
        void loadMail(renderToken);
      }
    }, 20_000);
  }

  async function loadMail(token: number, fresh = false): Promise<void> {
    try {
      const inboxes = fresh ? "/api/mail/api/inboxes?fresh=1" : "/api/mail/api/inboxes";
      const [status, list, acceptance] = await Promise.all([
        api<ServiceStatus>("/api/mail/api/status"),
        api<{ inboxes?: MailInbox[] }>(inboxes),
        api<{ provider_health?: ProviderHealth[] }>("/api/mail/api/acceptance").catch(() => null),
      ]);
      if (token !== renderToken || current !== "mail") return;
      mail.providers = status.providers ?? [];
      mail.limit = status.limits?.inboxes ?? 0;
      mail.healthUnavailable = !acceptance?.provider_health;
      if (acceptance?.provider_health) mail.health = acceptance.provider_health;
      mail.inboxes = list.inboxes ?? [];
      for (const inbox of mail.inboxes) {
        const previous = mail.unread[inbox.id];
        if (previous !== undefined && inbox.unread > previous) {
          toast(`New mail in ${inbox.address}`, 3_500);
        }
      }
      mail.unread = Object.fromEntries(
        mail.inboxes.map((inbox) => [inbox.id, inbox.unread]),
      );
      if (mail.selected && !mail.inboxes.some((inbox) => inbox.id === mail.selected)) {
        mail.selected = null;
        mail.detail = null;
        mail.messages = [];
      }
      paintMail();
      // A fresh inbox pass already refreshed the provider cache for every
      // inbox. Fetch from that cache rather than spending a second provider
      // request for the selected mailbox.
      if (mail.selected) void loadMailMessages();
    } catch (error) {
      if (token === renderToken && current === "mail") {
        app.innerHTML = `<div class="empty"><h2>Mail is unavailable</h2><p>${esc(String(error))}</p></div>`;
      }
    }
  }

  function paintMail(): void {
    // Providers can disappear between builds (the hosted API no longer offers
    // Mail.tm). Never keep a selection the server does not provide.
    if (mail.providers.length > 0 && !mail.providers.some((provider) => provider.id === mail.provider)) {
      mail.provider = mail.providers[0].id;
    }
    const picker = GmoUI.$<HTMLSelectElement>("#mail-provider");
    if (picker) {
      const options = mail.providers
        .map((provider) => `<option value="${esc(provider.id)}"${mail.provider === provider.id ? " selected" : ""}>${esc(provider.label)}</option>`)
        .join("");
      if (picker.innerHTML !== options) picker.innerHTML = options;
    }
    const selected = mail.providers.find((provider) => provider.id === mail.provider);
    const rule = GmoUI.$<HTMLElement>("#mail-provider-note");
    if (rule) {
      rule.textContent = selected?.address_rule ? `Address rule: ${selected.address_rule}` : "";
    }
    const cap = capState(mail.inboxes, mail.limit);
    const capNote = GmoUI.$<HTMLElement>("#mail-cap");
    if (capNote) {
      capNote.hidden = !cap.full;
      capNote.innerHTML = cap.html;
    }
    const newInbox = GmoUI.$<HTMLButtonElement>("#mail-new");
    if (newInbox) {
      newInbox.disabled = cap.full;
      newInbox.title = cap.full ? "Delete an inbox before creating another" : "";
    }
    const health = GmoUI.$<HTMLElement>("#mail-health");
    if (health) {
      const current = mail.health.filter((row) => row.error_is_current && row.last_error);
      const history = mail.health.filter((row) => row.create_failures > 0 || row.list_failures > 0);
      if (mail.healthUnavailable) {
        health.textContent = "Provider health is unavailable right now. Other Mail data may be stale.";
      } else if (current.length > 0) {
        health.textContent = `Provider issue now: ${current.map((row) => `${row.provider} — ${row.last_error}`).join(" · ")}`;
      } else if (history.length > 0) {
        const lastSuccess = history
          .map((row) => row.last_success_at)
          .filter((value) => value > 0)
          .sort((a, b) => b - a)[0];
        health.textContent = `Providers responding · ${history.map((row) => `${row.provider}: ${row.create_failures} create / ${row.list_failures} list failures`).join(" · ")}${lastSuccess ? ` · last success ${new Date(lastSuccess).toLocaleTimeString()}` : ""}`;
      } else if (mail.health.length > 0) {
        const checked = mail.health
          .map((row) => row.last_attempt_at || 0)
          .sort((a, b) => b - a)[0];
        const rows = mail.health
          .map((row) => `${row.provider}: ${row.error_is_current ? "failing" : "ok"}`)
          .join(" · ");
        health.textContent = `Provider health · ${rows}${checked ? ` · checked ${ageLabel(checked)}` : ""}`;
      } else {
        health.textContent = "Provider health: no failures recorded yet.";
      }
    }

    const list = GmoUI.$<HTMLElement>("#mail-list");
    if (list) {
      if (mail.inboxes.length === 0) {
        list.innerHTML = '<div class="pad muted">No inboxes yet. Use “New inbox”.</div>';
      } else {
        list.innerHTML = mail.inboxes
          .map((inbox) => {
            const last = inbox.last;
            const pill = inbox.unread > 0 ? `<span class="pill">${inbox.unread}</span>` : "";
            return `<button class="item ${mail.selected === inbox.id ? "on" : ""}" data-action="mail-select" data-id="${esc(inbox.id)}">
              <div class="item-top"><span class="item-who mono">${esc(inbox.address)}</span>${pill}</div>
              <div class="item-title">${esc(last?.subject || "Waiting for the first message")}</div>
              <div class="item-snips">${esc(last?.sender || inbox.provider)} · ${ttlSpan(inbox.expires_at)}</div>
            </button>`;
          })
          .join("");
      }
    }
    paintMailThread();
    updateTtls();
  }

  function paintMailThread(): void {
    const thread = GmoUI.$<HTMLElement>("#mail-thread");
    if (!thread) return;
    if (!mail.selected) {
      thread.innerHTML = '<div class="empty-reading"><p class="muted">Select an inbox, then a message.</p></div>';
      return;
    }
    const inbox = mail.inboxes.find((item) => item.id === mail.selected);
    if (!inbox) {
      thread.innerHTML = '<div class="empty-reading"><p class="muted">That inbox is gone.</p></div>';
      return;
    }
    const list = mail.messages.length === 0
      ? '<div class="pad muted">No messages yet. Refresh or wait for mail.</div>'
      : mail.messages
          .map(
            (message) => `<button class="item ${mail.detail?.id === message.id ? "on" : ""} ${message.seen ? "" : "unread"}" data-action="mail-open" data-id="${esc(message.id)}">
              <div class="item-top"><span class="item-who">${esc(message.sender)}</span><span class="item-when">${esc(GmoUI.messageTime(message.received_at))}</span></div>
              <div class="item-title">${esc(message.subject || "(no subject)")}</div>
              <div class="item-snips">${esc(message.preview || "")}</div>
            </button>`,
          )
          .join("");
    const detail = mail.detail && mail.detail.id && mail.messages.some((m) => m.id === mail.detail?.id)
      ? renderMailDetail(mail.detail)
      : '<div class="empty-reading"><p class="muted">Select a message to read it.</p></div>';
    thread.innerHTML = `
      <div class="row-between">
        <div>
          <span class="label">inbox</span>
          <div class="mono">${esc(inbox.address)}</div>
        </div>
        <div class="row">
          <button class="btn small" data-copy="${esc(inbox.address)}">Copy</button>
          <button class="btn small" data-action="mail-refresh">Refresh</button>
          <button class="btn small danger" data-action="mail-delete" data-id="${esc(inbox.id)}">Delete</button>
        </div>
      </div>
      <div class="row" style="margin:10px 0 14px"><span class="chip live">${esc(inbox.provider)}</span><span class="ttl">${ttlSpan(inbox.expires_at)}</span></div>
      <div class="split" style="border:0;min-height:0">
        <div class="split-list" style="border-right:1px solid var(--line);max-height:320px">${list}</div>
        <div style="padding:18px;overflow:auto;max-height:320px">${detail}</div>
      </div>`;
    updateTtls();
  }

  function renderMailDetail(detail: MailDetail): string {
    const code = detail.code
      ? `<div class="code-box"><div><div class="label">verification code</div><div class="value">${esc(detail.code)}</div></div><button class="btn" data-copy="${esc(detail.code)}">Copy code</button></div>`
      : "";
    const link = detail.link
      ? `<p><a href="${esc(detail.link)}" target="_blank" rel="noreferrer noopener">Open verification link</a> <button class="btn small" data-copy="${esc(detail.link)}">Copy link</button></p>`
      : "";
    const htmlUrl = `${API_BASE}/api/mail/api/messages/${encodeURIComponent(detail.id)}/html${WEB ? `?client=${encodeURIComponent(clientId())}` : ""}`;
    const html = detail.has_html
      ? `<details><summary class="muted small">HTML view</summary><iframe class="html-frame" sandbox title="Message HTML" src="${htmlUrl}"></iframe></details>`
      : "";
    return `<div class="read-head"><h2>${esc(detail.subject || "(no subject)")}</h2><div class="read-meta"><span>${esc(detail.sender)}</span><span>·</span><span>${esc(new Date(detail.received_at).toLocaleString())}</span></div></div>
      ${code}${link}<div class="body-text">${esc(detail.text || "")}</div>${html}`;
  }

  async function loadMailMessages(fresh = false): Promise<void> {
    const id = mail.selected;
    if (!id) return;
    try {
      const path = `/api/mail/api/inboxes/${encodeURIComponent(id)}/messages${fresh ? "?fresh=1" : ""}`;
      const data = await api<{ messages?: MailMessage[] }>(path);
      if (current !== "mail" || mail.selected !== id) return;
      mail.messages = data.messages ?? [];
      if (mail.detail && !mail.messages.some((message) => message.id === mail.detail?.id)) {
        mail.detail = null;
      }
      paintMailThread();
    } catch (error) {
      fail(error);
    }
  }

  async function openMailMessage(id: string): Promise<void> {
    try {
      const detail = await api<MailDetail>(`/api/mail/api/messages/${encodeURIComponent(id)}`);
      if (current !== "mail") return;
      const opened = mail.messages.find((message) => message.id === id);
      if (opened && !opened.seen) {
        opened.seen = true;
        const inbox = mail.inboxes.find((item) => item.id === mail.selected);
        if (inbox) inbox.unread = Math.max(0, inbox.unread - 1);
        paintMail();
      }
      mail.detail = detail;
      paintMailThread();
    } catch (error) {
      fail(error);
    }
  }

  async function createInbox(button: HTMLElement): Promise<void> {
    const picker = GmoUI.$<HTMLSelectElement>("#mail-provider");
    mail.provider = picker?.value || mail.provider;
    button.setAttribute("aria-busy", "true");
    (button as HTMLButtonElement).disabled = true;
    try {
      const created = await api<{ id?: string; provider?: string; requested_provider?: string; fallback?: boolean }>("/api/mail/api/inboxes", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ provider: mail.provider }),
      });
      if (!created.id) throw new Error("The provider did not return an inbox");
      if (created.fallback) {
        toast(`Fell back from ${created.requested_provider ?? mail.provider} to ${created.provider ?? "another provider"}.`);
      }
      mail.selected = created.id;
      await loadMail(renderToken);
    } catch (error) {
      if ((error as { code?: string }).code === "inbox_limit") {
        paintMail();
        toast(capMessage(mail.inboxes, mail.limit, "inboxes"));
      } else {
        fail(error);
      }
    } finally {
      (button as HTMLButtonElement).disabled = capState(mail.inboxes, mail.limit).full;
      button.removeAttribute("aria-busy");
    }
  }

  // ---------------------------------------------------------------- sms

  function countryLabel(slug: string): string {
    return slug.split("-").filter(Boolean).map((part) => part.charAt(0).toUpperCase() + part.slice(1)).join(" ");
  }

  function numberLabel(digits: string): string {
    return digits.startsWith("+") ? digits : `+${digits}`;
  }

  function ageLabel(at: number): string {
    const seconds = Math.max(0, Math.round((Date.now() - at) / 1000));
    if (seconds < 90) return "just now";
    const minutes = Math.round(seconds / 60);
    return minutes < 90 ? `${minutes}m ago` : `${Math.round(minutes / 60)}h ago`;
  }

  function preferSmsHealth(next?: SourceHealth | null): void {
    if (!next || !sms.status) return;
    const current = sms.status.source_health;
    // The status endpoint and the country endpoint can race. Keep the newer
    // attempt, or adopt the country result when status has nothing yet.
    if (!current || current.status === "unknown" || (next.last_attempt_at ?? 0) > (current.last_attempt_at ?? 0)) {
      sms.status.source_health = next;
    }
  }

  function renderSms(): void {
    const token = ++renderToken;
    app.innerHTML = `
      <section class="intro">
        <div><h1>SMS <span class="chip demo">Beta</span></h1><p class="muted">Browse a country's public numbers, then read their texts and codes.</p></div>
        <div class="intro-actions"><span class="chip live" id="sms-live">receive-sms.io</span></div>
      </section>
      <p class="note">Beta: these are public shared inboxes. Anyone who opens the same page sees the same texts, and upstream availability changes without notice. Pick a country first, then a listed number.</p>
      <form class="panel panel-form" id="sms-picker">
        <div class="panel-row"><label for="sms-country">Country</label><select class="select" id="sms-country"></select></div>
        <div class="panel-row"><label for="sms-number">Public number</label><select class="select" id="sms-number"></select></div>
        <div class="panel-row"><span></span><span class="row"><button class="btn" type="button" data-action="sms-check">Check source</button><button class="btn primary" id="sms-add" type="submit">Add selected number</button></span></div>
      </form>
      <p class="muted small" id="sms-source">Checking receive-sms.io…</p>
      <p class="note" id="sms-cap" hidden></p>
      <div class="split">
        <div class="split-list" id="sms-list"><div class="pad muted">No numbers loaded.</div></div>
        <div class="split-main" id="sms-thread"><div class="empty-reading"><p class="muted">Select a number to read its texts.</p></div></div>
      </div>
      ${WEB ? "" : `<div id="sms-webhook"></div>
      <details class="guide"><summary>Webhook and limits</summary>
        <p>Signed inbound texts: <code>POST /api/inbound/sms</code> on port 8782. Twilio uses <code>TWILIO_AUTH_TOKEN</code>, Telnyx uses <code>TELNYX_PUBLIC_KEY</code>, and anything else needs <code>WEBHOOK_SECRET</code>. An unset contract answers 503.</p>
        <p>A carrier retry is matched by its message id. A generic payload without an id is stored again on retry.</p>
      </details>`}`;
    void loadSms(token);
  }

  async function loadSms(token: number): Promise<void> {
    try {
      const [status, list, catalog] = await Promise.all([
        api<ServiceStatus>("/api/sms/api/status"),
        api<{ numbers?: SmsNumber[] }>("/api/sms/api/numbers"),
        api<{ countries?: string[]; source_health?: SourceHealth }>("/api/sms/api/countries").catch(() => null),
      ]);
      if (token !== renderToken || current !== "sms") return;
      sms.status = status;
      sms.limit = status.limits?.numbers ?? 0;
      sms.numbers = list.numbers ?? [];
      if (catalog?.countries) sms.countries = catalog.countries;
      preferSmsHealth(catalog?.source_health);
      if (!sms.country || sms.countries.indexOf(sms.country) < 0) {
        sms.country = sms.countries[0] ?? "";
      }
      if (sms.selected && !sms.numbers.some((number) => number.id === sms.selected)) {
        sms.selected = null;
        sms.messages = [];
        sms.detail = null;
      }
      paintSms();
      if (sms.country && sms.available.length === 0) void loadSmsNumbers();
      if (sms.selected) void loadSmsMessages();
    } catch (error) {
      if (token === renderToken && current === "sms") {
        app.innerHTML = `<div class="empty"><h2>SMS is unavailable</h2><p>${esc(String(error))}</p></div>`;
      }
    }
  }

  async function loadSmsNumbers(): Promise<void> {
    const country = sms.country;
    if (!country) return;
    sms.available = [];
    sms.chosen = "";
    paintSmsPicker();
    try {
      const data = await api<{ numbers?: string[]; source_health?: SourceHealth }>(`/api/sms/api/countries/${encodeURIComponent(country)}/numbers`);
      if (current !== "sms" || sms.country !== country) return;
      sms.available = data.numbers ?? [];
      if (sms.available.indexOf(sms.chosen) < 0) sms.chosen = sms.available[0] ?? "";
      preferSmsHealth(data.source_health);
      paintSms();
    } catch (error) {
      if (current === "sms" && sms.country === country) {
        sms.available = [];
        sms.chosen = "";
        await refreshSmsHealth();
        paintSms();
        fail(error);
      }
    }
  }

  async function refreshSmsHealth(): Promise<void> {
    try {
      const data = await api<{ source?: SourceHealth }>("/api/sms/api/health");
      if (current === "sms" && data.source && sms.status) {
        sms.status.source_health = data.source;
      }
    } catch {
      // The original source error is the useful message; do not replace it.
    }
  }

  async function checkSmsSource(): Promise<void> {
    try {
      const data = await api<{ countries?: string[]; source_health?: SourceHealth }>("/api/sms/api/source/check", { method: "POST" });
      if (current !== "sms") return;
      sms.countries = data.countries ?? sms.countries;
      preferSmsHealth(data.source_health);
      if (!sms.country || sms.countries.indexOf(sms.country) < 0) sms.country = sms.countries[0] ?? "";
      paintSms();
      await loadSmsNumbers();
      toast(`Source ${data.source_health?.status ?? "checked"} · ${sms.countries.length} countries`);
    } catch (error) {
      await refreshSmsHealth();
      paintSms();
      fail(error);
    }
  }

  function paintSms(): void {
    const badge = GmoUI.$<HTMLElement>("#sms-live");
    if (badge && sms.status?.source_health) {
      const source = sms.status.source_health;
      badge.textContent = `receive-sms.io · ${source.status}`;
      badge.className = `chip ${source.status === "ok" ? "live" : "demo"}`;
      badge.title = source.last_error || sms.status.provider_rule || "";
    }
    const source = GmoUI.$<HTMLElement>("#sms-source");
    if (source) {
      const health = sms.status?.source_health;
      if (!health) {
        source.textContent = "Source status not loaded yet.";
      } else if (health.last_source === "cache") {
        source.textContent = `Last good country list is being shown · ${health.last_error || "source unavailable"}`;
      } else if (health.last_error) {
        source.textContent = `Source ${health.status} · ${health.last_error}`;
      } else {
        const lastOk = health.last_success_at
          ? ` Last ok ${new Date(health.last_success_at).toLocaleTimeString()}.`
          : "";
        source.textContent = `Source ${health.status} · ${sms.countries.length} countries listed · public numbers are shared inboxes.${lastOk}`;
      }
    }
    const list = GmoUI.$<HTMLElement>("#sms-list");
    if (list) {
      list.innerHTML = sms.numbers.length === 0
        ? '<div class="pad muted">No numbers saved yet. Pick one above.</div>'
        : sms.numbers
            .map(
              (number) => `<button class="item ${sms.selected === number.id ? "on" : ""}" data-action="sms-select" data-id="${esc(number.id)}">
                <div class="item-top"><span class="item-who mono">${esc(number.e164)}</span></div>
                <div class="item-title">${esc(countryLabel(number.country))} · ${esc(number.provider)}</div>
                <div class="item-snips">Beta · public shared inbox · expires ${ttlSpan(number.expires_at)}</div>
              </button>`,
            )
            .join("");
    }
    const cap = capState(sms.numbers, sms.limit);
    const capNote = GmoUI.$<HTMLElement>("#sms-cap");
    if (capNote) {
      capNote.hidden = !cap.full;
      capNote.innerHTML = cap.html;
    }
    const addNumber = GmoUI.$<HTMLButtonElement>("#sms-add");
    if (addNumber) {
      addNumber.disabled = cap.full;
      addNumber.title = cap.full ? "Remove a number before adding another" : "";
    }
    paintSmsPicker();
    paintSmsThread();
    paintWebhook("#sms-webhook", sms.status);
    updateTtls();
  }

  function paintSmsPicker(): void {
    const country = GmoUI.$<HTMLSelectElement>("#sms-country");
    if (country) {
      country.innerHTML = sms.countries.length === 0
        ? '<option value="">No countries listed</option>'
        : sms.countries.map((slug) => `<option value="${esc(slug)}"${slug === sms.country ? " selected" : ""}>${esc(countryLabel(slug))}</option>`).join("");
      country.value = sms.country;
    }
    const number = GmoUI.$<HTMLSelectElement>("#sms-number");
    if (number) {
      number.innerHTML = sms.available.length === 0
        ? '<option value="">No numbers listed</option>'
        : sms.available.map((digits) => `<option value="${esc(digits)}"${digits === sms.chosen ? " selected" : ""}>${esc(numberLabel(digits))}</option>`).join("");
      number.value = sms.chosen;
    }
  }

  function paintSmsThread(): void {
    const thread = GmoUI.$<HTMLElement>("#sms-thread");
    if (!thread) return;
    if (!sms.selected) {
      thread.innerHTML = '<div class="empty-reading"><p class="muted">Select a number, then a text.</p></div>';
      return;
    }
    const number = sms.numbers.find((item) => item.id === sms.selected);
    if (!number) {
      thread.innerHTML = '<div class="empty-reading"><p class="muted">That number is gone.</p></div>';
      return;
    }
    const list = sms.messages.length === 0
      ? '<div class="pad muted">No texts yet. Refresh or wait for a delivery.</div>'
      : sms.messages
          .map(
            (message) => `<button class="item ${sms.detail?.id === message.id ? "on" : ""} ${message.seen ? "" : "unread"}" data-action="sms-open" data-id="${esc(message.id)}">
              <div class="item-top"><span class="item-who">${esc(message.sender)}</span><span class="item-when">${esc(GmoUI.messageTime(message.received_at))}</span></div>
              <div class="item-snips">${esc(message.preview || message.body || "")}</div>
            </button>`,
          )
          .join("");
    const detail = sms.detail && sms.messages.some((m) => m.id === sms.detail?.id)
      ? renderSmsDetail(sms.detail)
      : '<div class="empty-reading"><p class="muted">Select a text to read it.</p></div>';
    thread.innerHTML = `
      <div class="row-between"><div><span class="label">number</span><div class="mono">${esc(number.e164)}</div></div>
        <div class="row"><button class="btn small" data-copy="${esc(number.e164)}">Copy</button><button class="btn small" data-action="sms-refresh">Refresh</button><button class="btn small danger" data-action="sms-delete" data-id="${esc(number.id)}">Delete</button></div>
      </div>
      <div class="row" style="margin:10px 0 14px"><span class="chip live">${esc(number.provider)}</span><span class="ttl">${ttlSpan(number.expires_at)}</span></div>
      <div class="split" style="border:0;min-height:0">
        <div class="split-list" style="border-right:1px solid var(--line);max-height:320px">${list}</div>
        <div style="padding:18px;overflow:auto;max-height:320px">${detail}</div>
      </div>`;
    updateTtls();
  }

  function renderSmsDetail(detail: SmsDetail): string {
    const code = detail.code
      ? `<div class="code-box"><div><div class="label">code found</div><div class="value">${esc(detail.code)}</div></div><button class="btn" data-copy="${esc(detail.code)}">Copy code</button></div>`
      : "";
    return `<div class="bubble-meta"><span>${esc(detail.sender)}</span><span>·</span><span>${esc(new Date(detail.received_at).toLocaleString())}</span></div>${code}<div class="bubble">${esc(detail.body || "")}</div>`;
  }

  async function loadSmsMessages(fresh = false): Promise<void> {
    const id = sms.selected;
    if (!id) return;
    try {
      const data = await api<{ messages?: SmsMessage[] }>(`/api/sms/api/numbers/${encodeURIComponent(id)}/messages${fresh ? "?fresh=1" : ""}`);
      if (current !== "sms" || sms.selected !== id) return;
      sms.messages = data.messages ?? [];
      if (sms.detail && !sms.messages.some((message) => message.id === sms.detail?.id)) sms.detail = null;
      paintSmsThread();
    } catch (error) {
      fail(error);
    }
  }

  async function openSmsMessage(id: string): Promise<void> {
    try {
      const detail = await api<SmsDetail>(`/api/sms/api/messages/${encodeURIComponent(id)}`);
      if (current !== "sms") return;
      sms.detail = detail;
      paintSmsThread();
    } catch (error) {
      fail(error);
    }
  }

  async function addSmsNumber(form: HTMLFormElement): Promise<void> {
    const country = GmoUI.$<HTMLSelectElement>("#sms-country", form);
    const number = GmoUI.$<HTMLSelectElement>("#sms-number", form);
    const button = GmoUI.$<HTMLButtonElement>("button[type=submit]", form);
    if (!country || !number || !button) return;
    sms.country = country.value;
    sms.chosen = number.value;
    if (!sms.country || !sms.chosen) {
      toast("Choose a country and a listed public number first.");
      return;
    }
    button.disabled = true;
    try {
      const created = await api<{ id?: string }>("/api/sms/api/numbers", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ country: sms.country, number: sms.chosen }),
      });
      if (created.id) sms.selected = created.id;
      await loadSms(renderToken);
      void loadSmsMessages(true);
    } catch (error) {
      if ((error as { code?: string }).code === "number_limit") {
        paintSms();
        toast(capMessage(sms.numbers, sms.limit, "numbers"));
      } else {
        fail(error);
      }
    } finally {
      button.disabled = capState(sms.numbers, sms.limit).full;
    }
  }

  function paintWebhook(selector: string, status: ServiceStatus | null): void {
    if (WEB) return;
    const holder = GmoUI.$<HTMLElement>(selector);
    if (!holder) return;
    const hook = status?.webhook;
    if (!hook) {
      holder.innerHTML = "";
      return;
    }
    const mark = (ready: boolean) => ready ? '<span class="chip live">configured</span>' : '<span class="chip demo">not set</span>';
    const ready = status?.webhook_configured ?? (hook.twilio || hook.telnyx || hook.generic);
    holder.innerHTML = `<div class="panel"><div class="row-between"><strong>Webhook contracts</strong><span class="muted small">${ready ? "ready" : "configure a contract before exposing the endpoint"}</span></div>
      <div class="panel-row"><span>Twilio</span><span>${mark(hook.twilio)} <code>TWILIO_AUTH_TOKEN</code></span></div>
      <div class="panel-row"><span>Telnyx</span><span>${mark(hook.telnyx)} <code>TELNYX_PUBLIC_KEY</code></span></div>
      <div class="panel-row"><span>Generic</span><span>${mark(hook.generic)} <code>WEBHOOK_SECRET</code></span></div>
    </div>`;
  }

  // ---------------------------------------------------------------- otp

  function renderOtp(): void {
    const token = ++renderToken;
    app.innerHTML = `
      <section class="intro">
        <div><h1>OTP</h1><p class="muted">Read a code out of pasted text, or generate authenticator codes.</p></div>
        <div class="intro-actions"><span class="chip live">offline core</span></div>
      </section>
      <p class="note">The extraction and TOTP engine runs in this browser. ${WEB ? "Authenticator secrets are stored only in this browser's localStorage; they are never sent to the web API." : "Authenticator secrets live in <code>temp-otp/state/vault.json</code> and are shared with the desktop tab."}</p>
      <div class="otp-tabs">
        <button class="otp-tab ${otp.tab === "extract" ? "on" : ""}" data-action="otp-tab" data-tab="extract">Read a code</button>
        <button class="otp-tab ${otp.tab === "generate" ? "on" : ""}" data-action="otp-tab" data-tab="generate">Authenticator</button>
      </div>
      <div id="otp-body"></div>
              <details class="guide"><summary>Security model</summary><p>${WEB ? "Secrets never leave this browser. They are stored in localStorage without the desktop app's DPAPI encryption, so use this browser profile carefully and keep another encrypted backup of any 2FA secret you cannot afford to lose — clearing site data or switching browsers otherwise loses the vault." : "Secrets never leave this machine. On Windows the vault is encrypted at rest with DPAPI for your Windows user and writes are serialized with the desktop tab. Do not use this as your only copy of an account's 2FA secret."}</p></details>`;
    if (otp.tab === "extract") {
      paintOtpExtract();
      void loadOtp(token);
    } else {
      paintOtpGenerate();
      void loadOtp(token);
      addTimer(() => void loadOtp(renderToken), 1000);
    }
  }

  function paintOtpExtract(): void {
    const body = GmoUI.$<HTMLElement>("#otp-body");
    if (!body) return;
    body.innerHTML = `<div class="panel">
      <label class="label" for="otp-text">Paste a verification email or text</label>
      <textarea class="textarea" id="otp-text" placeholder="Your verification code is 481920…">${esc(otp.paste)}</textarea>
      <div class="row" style="margin-top:10px"><button class="btn primary" data-action="otp-extract">Find codes</button><button class="btn" data-action="otp-clear">Clear</button><span class="muted small">Codes near words like code, OTP or verification are ranked first.</span></div>
    </div>
    <div id="otp-found">${renderFound()}</div>`;
  }

  function paintOtpGenerate(): void {
    const body = GmoUI.$<HTMLElement>("#otp-body");
    if (!body) return;
    body.innerHTML = `<form class="panel panel-form" id="otp-add">
      <div class="panel-row"><label for="otp-uri">otpauth:// URI or base32 secret</label><input class="input" id="otp-uri" value="${esc(otp.uri)}" placeholder="otpauth://totp/… or JBSWY3DPEHPK3PXP"></div>
      <div class="panel-row"><label for="otp-label">Label (optional)</label><input class="input" id="otp-label" value="${esc(otp.label)}" placeholder="Work account"></div>
      <div class="panel-row"><span></span><button class="btn primary" type="submit">Add account</button></div>
    </form>
    <div id="otp-secrets"></div>
    ${WEB ? otpBackupPanel() : ""}`;
    paintOtpSecrets();
  }

  function otpBackupPanel(): string {
    return `<div class="panel" id="otp-backup">
      <div class="row-between"><strong>Encrypted backup</strong><span class="chip" id="otp-backup-age">no backup yet</span></div>
      <p class="muted small">Export a passphrase-encrypted file before clearing site data or switching browsers. Encryption (PBKDF2 + AES-GCM) runs here; the passphrase and the backup never leave this browser. Without the passphrase the file cannot be opened.</p>
      <div class="panel-row"><label for="otp-export-pass">Export passphrase</label><input class="input" id="otp-export-pass" type="password" autocomplete="new-password" placeholder="At least 8 characters"></div>
      <div class="panel-row"><label for="otp-export-confirm">Confirm passphrase</label><input class="input" id="otp-export-confirm" type="password" autocomplete="new-password"></div>
      <div class="panel-row"><span></span><button class="btn" data-action="otp-export">Download encrypted backup</button></div>
      <div class="panel-row"><label for="otp-import-file">Restore from file</label><input class="input" id="otp-import-file" type="file" accept="application/json,.json"></div>
      <label class="label" for="otp-import-text">or paste the backup JSON</label>
      <textarea class="textarea" id="otp-import-text" placeholder="Paste the contents of a gmo-temptools OTP backup file"></textarea>
      <div class="panel-row"><label for="otp-import-pass">Restore passphrase</label><input class="input" id="otp-import-pass" type="password" autocomplete="current-password"></div>
      <div class="panel-row"><span></span><button class="btn" data-action="otp-import">Import and merge</button></div>
      <p class="muted small" id="otp-backup-status" role="status"></p>
    </div>`;
  }

  function setBackupStatus(text: string): void {
    const status = GmoUI.$<HTMLElement>("#otp-backup-status");
    if (status) status.textContent = text;
  }

  async function exportOtpVault(): Promise<void> {
    if (!WEB) return;
    const pass = GmoUI.$<HTMLInputElement>("#otp-export-pass")?.value ?? "";
    const confirmation = GmoUI.$<HTMLInputElement>("#otp-export-confirm")?.value ?? "";
    if (pass !== confirmation) {
      setBackupStatus("The two passphrases do not match.");
      return;
    }
    try {
      const backup = await GmoUI.encryptOtpBackup(loadLocalSecrets(), pass);
      const blob = new Blob([JSON.stringify(backup, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = `gmo-temptools-otp-backup-${new Date().toISOString().slice(0, 10)}.json`;
      document.body.appendChild(link);
      link.click();
      link.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
      const passInput = GmoUI.$<HTMLInputElement>("#otp-export-pass");
      const confirmInput = GmoUI.$<HTMLInputElement>("#otp-export-confirm");
      if (passInput) passInput.value = "";
      if (confirmInput) confirmInput.value = "";
      try {
        localStorage.setItem(LOCAL_OTP_BACKUP_KEY, String(Date.now()));
      } catch {
        // A private window may refuse storage; the download itself still works.
      }
      paintOtpSecrets();
      setBackupStatus(`Encrypted backup of ${backup.count} account(s) downloaded. Keep the passphrase safe; it cannot be recovered.`);
      toast("Encrypted backup downloaded.");
    } catch (error) {
      setBackupStatus(error instanceof Error ? error.message : String(error));
    }
  }

  async function importOtpVault(): Promise<void> {
    if (!WEB) return;
    const pass = GmoUI.$<HTMLInputElement>("#otp-import-pass")?.value ?? "";
    const fileInput = GmoUI.$<HTMLInputElement>("#otp-import-file");
    const file = fileInput?.files?.[0] ?? null;
    const pasted = GmoUI.$<HTMLTextAreaElement>("#otp-import-text")?.value.trim() ?? "";
    try {
      let text = pasted;
      if (file) {
        if (file.size > 2 * 1024 * 1024) throw new Error("That backup file is too large.");
        text = await file.text();
      }
      if (!text) throw new Error("Choose a backup file or paste its JSON first.");
      let payload: unknown;
      try {
        payload = JSON.parse(text);
      } catch {
        throw new Error("That backup is not valid JSON.");
      }
      const incoming = await GmoUI.decryptOtpBackup(payload, pass);
      const secrets = loadLocalSecrets();
      let added = 0;
      let present = 0;
      for (const secret of incoming) {
        if (secrets.some((item) => item.secret === secret.secret && item.label === secret.label)) {
          present += 1;
          continue;
        }
        if (secrets.length >= GmoUI.OTP_BACKUP_MAX_SECRETS) break;
        secrets.push(secret);
        added += 1;
      }
      if (added > 0) saveLocalSecrets(secrets);
      if (fileInput) fileInput.value = "";
      const textInput = GmoUI.$<HTMLTextAreaElement>("#otp-import-text");
      if (textInput) textInput.value = "";
      const passInput = GmoUI.$<HTMLInputElement>("#otp-import-pass");
      if (passInput) passInput.value = "";
      if (added > 0) await loadOtp(renderToken);
      setBackupStatus(
        added === 0
          ? `No new accounts imported; ${present} already present.`
          : `Imported ${added} account(s)${present > 0 ? `; ${present} already present` : ""}.`,
      );
      if (added > 0) toast(`Imported ${added} authenticator account(s).`);
    } catch (error) {
      setBackupStatus(error instanceof Error ? error.message : String(error));
    }
  }

  function renderFound(): string {
    if (!otp.found || (otp.found.codes.length === 0 && otp.found.links.length === 0)) {
      return '<div class="empty"><p>Paste some text and press “Find codes”.</p></div>';
    }
    const codes = otp.found.codes
      .map((code) => `<div class="code-box found"><div><div class="label">candidate code</div><div class="value">${esc(code)}</div></div><button class="btn" data-copy="${esc(code)}">Copy</button></div>`)
      .join("");
    const links = otp.found.links
      .map((link) => `<p><a href="${esc(link)}" target="_blank" rel="noreferrer noopener">${esc(link)}</a> <button class="btn small" data-copy="${esc(link)}">Copy</button></p>`)
      .join("");
    return `${codes}${links}`;
  }

  function paintFound(): void {
    const holder = GmoUI.$<HTMLElement>("#otp-found");
    if (holder) holder.innerHTML = renderFound();
  }

  function paintOtpSecrets(): void {
    const holder = GmoUI.$<HTMLElement>("#otp-secrets");
    if (!holder) return;
    holder.innerHTML = otp.secrets.length === 0
      ? '<div class="empty"><p>No authenticator accounts yet.</p></div>'
      : otp.secrets
          .map((secret) => {
            const pct = Math.max(0, Math.min(100, (secret.remaining / Math.max(1, secret.period)) * 100));
            return `<div class="secret"><div class="row-between">
              <div><div><strong>${esc(secret.label)}</strong></div><div class="muted small">${esc(secret.algorithm)} · ${secret.digits} digits · ${secret.period}s · ${esc(secret.masked)}</div></div>
              <div class="row"><span class="secret-code">${esc(secret.code)}</span><button class="btn small" data-copy="${esc(secret.code)}">Copy</button>${WEB ? `<button class="btn small" data-action="otp-rename" data-index="${secret.index}" title="Rename this account">Rename</button><button class="btn small" data-action="otp-move" data-index="${secret.index}" data-id="up" title="Move up" ${secret.index === 0 ? "disabled" : ""}>↑</button><button class="btn small" data-action="otp-move" data-index="${secret.index}" data-id="down" title="Move down" ${secret.index === otp.secrets.length - 1 ? "disabled" : ""}>↓</button>` : ""}<button class="btn small danger" data-action="otp-remove" data-index="${secret.index}">Remove</button></div>
            </div><div class="progress"><span style="width:${pct}%"></span></div><div class="muted small" style="margin-top:4px">${secret.remaining}s left</div></div>`;
          })
          .join("");
    const backupAge = GmoUI.$<HTMLElement>("#otp-backup-age");
    if (backupAge) {
      let backedUpAt = 0;
      try {
        backedUpAt = Number(localStorage.getItem(LOCAL_OTP_BACKUP_KEY) || 0);
      } catch {
        backedUpAt = 0;
      }
      if (otp.secrets.length === 0) {
        backupAge.textContent = "no accounts";
        backupAge.className = "chip";
      } else if (!backedUpAt) {
        backupAge.textContent = "backup needed";
        backupAge.className = "chip demo";
        backupAge.title = "Export an encrypted backup before clearing site data.";
      } else {
        backupAge.textContent = `backup ${GmoUI.age(backedUpAt)}`;
        backupAge.className = "chip live";
        backupAge.title = `Last encrypted export ${new Date(backedUpAt).toLocaleString()}`;
      }
    }
  }

  async function loadOtp(token: number): Promise<void> {
    try {
      const data = await api<{ secrets?: OtpSecret[] }>("/api/otp");
      if (token !== renderToken || current !== "otp") return;
      otp.secrets = data.secrets ?? [];
      paintOtpSecrets();
    } catch (error) {
      fail(error);
    }
  }

  async function extractOtp(): Promise<void> {
    const text = GmoUI.$<HTMLTextAreaElement>("#otp-text")?.value ?? otp.paste;
    otp.paste = text;
    try {
      otp.found = await api<FoundCodes>("/api/otp/extract", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text }),
      });
      paintFound();
    } catch (error) {
      fail(error);
    }
  }

  async function addOtp(form: HTMLFormElement): Promise<void> {
    const uri = GmoUI.$<HTMLInputElement>("#otp-uri", form);
    const label = GmoUI.$<HTMLInputElement>("#otp-label", form);
    if (!uri || !label) return;
    otp.uri = uri.value;
    otp.label = label.value;
    try {
      await api<{ ok?: boolean }>("/api/otp", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ input: otp.uri, label: otp.label }),
      });
      otp.uri = "";
      otp.label = "";
      renderOtp();
    } catch (error) {
      fail(error);
    }
  }

  // -------------------------------------------------------------- settings

  function settingsShell(): string {
    return `
      <section class="intro">
        <div><h1>Settings</h1><p class="muted">${WEB ? "Appearance, hosted access, release notes and data cleanup." : "Appearance, local paths, release notes and data cleanup."}</p></div>
        <div class="intro-actions"><span class="chip live">owner ${esc(settingsState.owner || "drifter1987")}</span></div>
      </section>
      <p class="note">${WEB ? "This hosted build keeps Mail/SMS on the web API and OTP in this browser. There is no account, subscription or remote license server." : "GMO TempTools is local-only. There is no account, subscription or remote license server in this build."}</p>
      <div class="panel">
        <div class="panel-row"><span>Appearance</span><span><select class="select" id="appearance" aria-label="Appearance"><option value="dark">Dark</option><option value="light">Light</option></select></span></div>
        <div class="panel-row"><span>Owner</span><span id="settings-owner">${esc(settingsState.owner)}</span></div>
        <div class="panel-row"><span>Version</span><span id="settings-version">${esc(settingsState.version)}</span></div>
      </div>
      <div class="panel">
        <div class="row-between"><strong>${WEB ? "Notification preference" : "Desktop notifications"}</strong><span class="chip">${WEB ? "stored preference" : "native only"}</span></div>
        <p class="muted small">${WEB ? "The hosted web build stores this preference, but cannot show the desktop app's native toasts. OTP codes are never notified." : "Toasts run in the desktop app while its window is hidden. OTP codes are never notified."}</p>
        <div class="panel-row"><label for="notify-enabled">Show desktop notifications</label><input type="checkbox" id="notify-enabled"></div>
        <div class="panel-row"><label for="notify-mail">New mail</label><input type="checkbox" id="notify-mail"></div>
        <div class="panel-row"><label for="notify-sms">New texts</label><input type="checkbox" id="notify-sms"></div>
        <div class="panel-row"><span></span><button class="btn" data-action="settings-test">Send test notification</button></div>
      </div>
      ${WEB ? `<div class="panel">
        <div class="row-between"><strong>Hosted access</strong><span class="chip live">web build</span></div>
        <p class="muted small">Mail and SMS data are stored on the web API for this browser. OTP secrets never leave this browser. There is no installation key, account or remote license in this build.</p>
      </div>` : `<div class="panel">
        <div class="row-between"><strong>Installation key and access</strong><span class="chip">per install</span></div>
        <p class="muted small">The key unlocks only this PC's local web app and APIs. Each installation creates its own key on first launch. To let someone use the app on another PC, install it there — there is no central key issuer or user list in this build.</p>
        <div id="settings-key" class="muted small">Loading key path…</div>
      </div>
      <div class="panel">
        <div class="row-between"><strong>Local paths</strong><span class="muted small">stored on this device only</span></div>
        <div id="settings-paths"><p class="muted small">Loading paths…</p></div>
      </div>`}
      ${WEB ? `<div class="panel">
        <div class="row-between"><strong>Upstream status</strong><span class="chip" id="settings-health-chip">…</span></div>
        <p class="muted small">Recorded by the hosted API. Mail falls back automatically when a provider fails; a warning row is failing right now.</p>
        <div id="settings-health"><p class="muted small">Loading provider health…</p></div>
      </div>` : ""}
      <h2 class="settings-h2">Release notes and changelog</h2>
      <div id="settings-notes"><p class="muted small">Loading release notes…</p></div>
      <div class="panel danger-panel">
        <div class="row-between"><strong>Clear all data</strong><span class="chip demo">destructive</span></div>
        <p class="muted small">${WEB ? "Deletes the hosted Mail/SMS metadata for this browser and the OTP/settings data stored here. Provider data and carrier billing are outside this app." : "Deletes Mail, SMS, the OTP vault, settings, the installation key and logs. Provider data and carrier billing are outside this app. Restart afterward."}</p>
        <button class="btn danger" data-action="settings-clear">Clear all data…</button>
      </div>
      <p class="muted small" id="settings-status">${esc(settingsState.status)}</p>`;
  }

  function renderSettings(): void {
    app.innerHTML = settingsShell();
    void loadSettings();
  }

  async function loadSettings(): Promise<void> {
    try {
      const data = await api<AppSettings>("/api/settings");
      if (current !== "settings") return;
      settingsState.appearance = data.appearance === "light" ? "light" : "dark";
      settingsState.notifications = data.notifications ?? settingsState.notifications;
      settingsState.owner = data.owner ?? "";
      settingsState.version = data.version ?? "";
      settingsState.paths = data.paths ?? [];
      settingsState.notes = data.release_notes ?? [];
      if (WEB) {
        const health = await api<{ providers?: HealthProvider[] }>("/api/health").catch(
          () => null,
        );
        settingsState.health = health?.providers ?? [];
      }
      paintSettings();
    } catch (error) {
      fail(error);
    }
  }

  function paintSettings(): void {
    document.documentElement.dataset.theme = settingsState.appearance;
    const picker = GmoUI.$<HTMLSelectElement>("#appearance");
    if (picker) picker.value = settingsState.appearance;
    const enabled = GmoUI.$<HTMLInputElement>("#notify-enabled");
    if (enabled) enabled.checked = settingsState.notifications.enabled;
    const mailNotify = GmoUI.$<HTMLInputElement>("#notify-mail");
    if (mailNotify) {
      mailNotify.checked = settingsState.notifications.mail;
      mailNotify.disabled = !settingsState.notifications.enabled;
    }
    const smsNotify = GmoUI.$<HTMLInputElement>("#notify-sms");
    if (smsNotify) {
      smsNotify.checked = settingsState.notifications.sms;
      smsNotify.disabled = !settingsState.notifications.enabled;
    }
    const owner = GmoUI.$<HTMLElement>("#settings-owner");
    if (owner) owner.textContent = settingsState.owner || "drifter1987";
    const version = GmoUI.$<HTMLElement>("#settings-version");
    if (version) version.textContent = settingsState.version || "—";
    const key = GmoUI.$<HTMLElement>("#settings-key");
    const keyPath = settingsState.paths.find((row) => row.label === "Installation key");
    if (key) {
      key.innerHTML = keyPath
        ? `<div class="path-row"><code>${esc(keyPath.path)}</code><button class="btn small" data-copy="${esc(keyPath.path)}">Copy path</button></div><p class="muted small">Use <code>./run-all.sh token</code> only in a development checkout; an installed app shows the same key in Settings.</p>`
        : "Key path unavailable.";
    }
    const paths = GmoUI.$<HTMLElement>("#settings-paths");
    if (paths) {
      paths.innerHTML = settingsState.paths.length === 0
        ? '<p class="muted small">No paths reported.</p>'
        : settingsState.paths
            .map((row) => `<div class="path-row"><span>${esc(row.label)}</span><code>${esc(row.path)}</code><button class="btn small" data-copy="${esc(row.path)}">Copy</button></div>`)
            .join("");
    }
    const healthHolder = GmoUI.$<HTMLElement>("#settings-health");
    if (healthHolder) {
      healthHolder.innerHTML = settingsState.health.length === 0
        ? '<p class="muted small">No health snapshot yet. The API records one when its probe runs.</p>'
        : settingsState.health
            .map((provider) => {
              const chip =
                provider.status === "ok" ? "live" : provider.status === "error" ? "demo" : "";
              return `<div class="path-row"><span>${esc(provider.label || provider.provider)}</span><span class="chip ${chip}">${esc(provider.status)}</span><code>${esc(provider.status === "error" ? provider.last_error || "failing" : provider.kind)}</code><span class="muted small">${provider.last_attempt_at ? GmoUI.age(provider.last_attempt_at) : "never"}</span></div>`;
            })
            .join("");
    }
    const healthChip = GmoUI.$<HTMLElement>("#settings-health-chip");
    if (healthChip) {
      const failing = settingsState.health.filter((provider) => provider.status === "error").length;
      healthChip.textContent =
        settingsState.health.length === 0 ? "no data" : failing > 0 ? `${failing} failing` : "all ok";
      healthChip.className = `chip ${failing > 0 ? "demo" : settingsState.health.length > 0 ? "live" : ""}`;
    }
    const notes = GmoUI.$<HTMLElement>("#settings-notes");
    if (notes) {
      notes.innerHTML = settingsState.notes.length === 0
        ? '<p class="muted small">No release notes reported.</p>'
        : settingsState.notes
            .map(
              (note) => `<details class="guide"><summary><strong>${esc(note.tool)} ${esc(note.version)}</strong> — ${esc(note.summary)}</summary><ol>${note.changelog
                .map((entry) => `<li><span class="mono">${esc(entry.version)}</span> · ${esc(entry.change)}</li>`)
                .join("")}</ol></details>`,
            )
            .join("");
    }
    const status = GmoUI.$<HTMLElement>("#settings-status");
    if (status) status.textContent = settingsState.status;
  }

  async function changeAppearance(appearance: "dark" | "light"): Promise<void> {
    try {
      const data = await api<AppSettings>("/api/settings", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ appearance }),
      });
      settingsState.appearance = data.appearance === "light" ? "light" : "dark";
      paintSettings();
      toast(`Appearance set to ${settingsState.appearance}.`);
    } catch (error) {
      paintSettings();
      fail(error);
    }
  }

  async function saveNotificationSettings(): Promise<void> {
    const enabled = GmoUI.$<HTMLInputElement>("#notify-enabled");
    const mail = GmoUI.$<HTMLInputElement>("#notify-mail");
    const sms = GmoUI.$<HTMLInputElement>("#notify-sms");
    if (!enabled || !mail || !sms) return;
    try {
      const data = await api<AppSettings>("/api/settings", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ notifications: { enabled: enabled.checked, mail: mail.checked, sms: sms.checked } }),
      });
      settingsState.notifications = data.notifications ?? settingsState.notifications;
      paintSettings();
      toast("Notification preferences saved.");
    } catch (error) {
      // Revert the controls to the last settings known to be on disk.
      paintSettings();
      fail(error);
    }
  }

  async function testNotification(): Promise<void> {
    if (WEB) {
      toast("Native desktop notifications are not part of the hosted web build.");
      return;
    }
    try {
      await api<{ ok: boolean }>("/api/notifications/test", { method: "POST" });
      toast("Test notification sent.");
    } catch (error) {
      fail(error);
    }
  }

  async function clearAllData(button: HTMLButtonElement): Promise<void> {
    const message = WEB
      ? "Delete the hosted Mail/SMS data for this browser and the OTP/settings stored in this browser? This cannot be undone."
      : "Delete all local GMO TempTools data, including the OTP vault and installation key? This cannot be undone.";
    if (!confirm(message)) return;
    button.disabled = true;
    try {
      await api<{ ok: boolean; restart_required: boolean }>("/api/clear-data", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ confirm: "CLEAR" }),
      });
      if (WEB) {
        try {
          localStorage.removeItem(LOCAL_OTP_KEY);
          localStorage.removeItem(CLIENT_ID_KEY);
        } catch {
          // Storage may be unavailable in a private window.
        }
      }
      settingsState.status = WEB
        ? "Hosted data cleared. Reload the page to start fresh."
        : "All local data cleared. Restart GMO TempTools to create a new installation key.";
      paintSettings();
      toast(WEB ? "Hosted data cleared. Reload the page." : "Local data cleared. Restart the app.", 5000);
    } catch (error) {
      fail(error);
    } finally {
      button.disabled = false;
    }
  }

  // ---------------------------------------------------------------- actions

  async function handleAction(action: string, id: string, index: number): Promise<void> {
    if (current === "settings") {
      if (action === "settings-clear") {
        const button = GmoUI.$<HTMLButtonElement>('[data-action="settings-clear"]');
        if (button) await clearAllData(button);
      }
      if (action === "settings-test") await testNotification();
      return;
    }
    if (current === "mail") {
      if (action === "mail-new") return createInbox(GmoUI.$<HTMLElement>('[data-action="mail-new"]') as HTMLElement);
      if (action === "mail-select") {
        mail.selected = id;
        mail.detail = null;
        mail.messages = [];
        paintMailThread();
        void loadMailMessages();
        return;
      }
      if (action === "mail-open") return openMailMessage(id);
      if (action === "mail-refresh") {
        await loadMail(renderToken, true);
        return;
      }
      if (action === "mail-delete") {
        if (!confirm("Delete this inbox and its messages?")) return;
        await api(`/api/mail/api/inboxes/${encodeURIComponent(id)}`, { method: "DELETE" }).catch(() => null);
        mail.selected = null;
        mail.messages = [];
        mail.detail = null;
        await loadMail(renderToken);
        return;
      }
    }
    if (current === "sms") {
      if (action === "sms-check") return checkSmsSource();
      if (action === "sms-select") {
        sms.selected = id;
        sms.detail = null;
        sms.messages = [];
        paintSmsThread();
        void loadSmsMessages();
        return;
      }
      if (action === "sms-open") return openSmsMessage(id);
      if (action === "sms-refresh") {
        await loadSmsMessages(true);
        return;
      }
      if (action === "sms-delete") {
        if (!confirm("Delete this number and its messages?")) return;
        await api(`/api/sms/api/numbers/${encodeURIComponent(id)}`, { method: "DELETE" }).catch(() => null);
        sms.selected = null;
        sms.messages = [];
        sms.detail = null;
        await loadSms(renderToken);
        return;
      }
    }
    if (current === "otp") {
      if (action === "otp-tab") {
        const tab = (id || "extract") as "extract" | "generate";
        otp.tab = tab;
        renderOtp();
        return;
      }
      if (action === "otp-extract") return extractOtp();
      if (action === "otp-clear") {
        otp.paste = "";
        otp.found = null;
        paintOtpExtract();
        return;
      }
      if (action === "otp-remove") {
        if (!confirm("Remove this authenticator account?")) return;
        await api(`/api/otp/${index}`, { method: "DELETE" }).catch(() => null);
        await loadOtp(renderToken);
        return;
      }
      if (action === "otp-rename") {
        if (!WEB) return;
        const secrets = loadLocalSecrets();
        const currentLabel = secrets[index]?.label ?? "";
        const next = window.prompt("Rename authenticator account", currentLabel);
        if (next === null) return;
        const clean = next.trim().slice(0, 80);
        if (!clean) {
          toast("A label is required.");
          return;
        }
        if (secrets[index]) {
          secrets[index].label = clean;
          saveLocalSecrets(secrets);
          await loadOtp(renderToken);
        }
        return;
      }
      if (action === "otp-move") {
        if (!WEB) return;
        const secrets = loadLocalSecrets();
        const target = index + (id === "up" ? -1 : 1);
        if (index >= 0 && index < secrets.length && target >= 0 && target < secrets.length) {
          const [moved] = secrets.splice(index, 1);
          secrets.splice(target, 0, moved);
          saveLocalSecrets(secrets);
          await loadOtp(renderToken);
        }
        return;
      }
      if (action === "otp-export") return exportOtpVault();
      if (action === "otp-import") return importOtpVault();
    }
  }

  app.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const copyTarget = target.closest<HTMLElement>("[data-copy]");
    if (copyTarget) {
      event.preventDefault();
      const value = copyTarget.dataset.copy;
      if (value) void copy(value);
      return;
    }
    const actionTarget = target.closest<HTMLElement>("[data-action]");
    if (!actionTarget) return;
    event.preventDefault();
    const action = actionTarget.dataset.action;
    if (!action) return;
    const id = actionTarget.dataset.id ?? actionTarget.dataset.tab ?? "";
    const index = Number(actionTarget.dataset.index ?? "-1");
    void handleAction(action, id, index);
  });

  app.addEventListener("submit", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLFormElement)) return;
    event.preventDefault();
    if (target.id === "sms-picker") void addSmsNumber(target);
    if (target.id === "otp-add") void addOtp(target);
  });

  app.addEventListener("change", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLSelectElement)) return;
    if (target.id === "sms-country") {
      sms.country = target.value;
      void loadSmsNumbers();
    }
    if (target.id === "sms-number") sms.chosen = target.value;
    if (target.id === "mail-provider") {
      mail.provider = target.value;
      paintMail();
    }
    if (target.id === "appearance" && (target.value === "dark" || target.value === "light")) {
      void changeAppearance(target.value);
    }
  });

  app.addEventListener("change", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLInputElement) || target.type !== "checkbox") return;
    if (target.id === "notify-enabled" || target.id === "notify-mail" || target.id === "notify-sms") {
      void saveNotificationSettings();
    }
  });

  app.addEventListener("input", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)) return;
    if (target.id === "otp-text") otp.paste = target.value;
    if (target.id === "otp-uri") otp.uri = target.value;
    if (target.id === "otp-label") otp.label = target.value;
    if (target.id === "mail-provider") mail.provider = target.value;
  });

  document.querySelectorAll<HTMLButtonElement>("#main-tabs .main-tab").forEach((button) => {
    button.addEventListener("click", () => {
      const screen = button.dataset.screen ?? "";
      if (isScreen(screen)) show(screen);
    });
  });

  window.setInterval(updateTtls, 1000);
  const initial = window.location.hash.replace("#", "");
  show(isScreen(initial) ? initial : "mail");
})();
