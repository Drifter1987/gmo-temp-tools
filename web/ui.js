"use strict";
/**
 * Shared browser helpers for the three GMO temporary-services frontends.
 *
 * Compiled to /ui.js and loaded before each app's /app.js. This is a global
 * namespace on purpose: the apps ship standalone scripts with no bundler.
 */
var GmoUI;
(function (GmoUI) {
    function $(selector, root = document) {
        return root.querySelector(selector);
    }
    GmoUI.$ = $;
    function esc(value) {
        return String(value ?? "").replace(/[&<>"']/g, (character) => ({
            "&": "&amp;",
            "<": "&lt;",
            ">": "&gt;",
            '"': "&quot;",
            "'": "&#39;",
        })[character] ?? character);
    }
    GmoUI.esc = esc;
    let toastTimer;
    function toast(text, timeout = 1600) {
        let element = $(".toast");
        if (!element) {
            element = document.createElement("div");
            element.className = "toast";
            element.setAttribute("role", "status");
            element.setAttribute("aria-live", "polite");
            document.body.appendChild(element);
        }
        element.textContent = text;
        element.classList.add("show");
        window.clearTimeout(toastTimer);
        toastTimer = window.setTimeout(() => element?.classList.remove("show"), timeout);
    }
    GmoUI.toast = toast;
    async function copy(text, timeout = 1600) {
        try {
            await navigator.clipboard.writeText(text);
            toast("Copied", timeout);
        }
        catch {
            const textarea = document.createElement("textarea");
            textarea.value = text;
            document.body.appendChild(textarea);
            textarea.select();
            document.execCommand("copy");
            textarea.remove();
            toast("Copied", timeout);
        }
    }
    GmoUI.copy = copy;
    async function readJson(response) {
        try {
            return (await response.json());
        }
        catch {
            return null;
        }
    }
    GmoUI.readJson = readJson;
    function updateTtls(format) {
        const now = Date.now();
        document.querySelectorAll("[data-ttl]").forEach((element) => {
            const holder = element.closest("[data-expires]") ?? element;
            const expires = Number(holder.dataset.expires ?? element.dataset.expires ?? 0);
            if (!expires)
                return;
            element.textContent = format(expires - now);
        });
    }
    GmoUI.updateTtls = updateTtls;
    function ttl(remainingMs, compact = false) {
        if (!Number.isFinite(remainingMs))
            return "";
        const mins = Math.max(0, Math.ceil(remainingMs / 60000));
        if (mins <= 0)
            return "expired";
        if (mins < 60)
            return compact ? `${mins}m left` : `${mins} min left`;
        if (mins < 1440)
            return `${Math.floor(mins / 60)}h left`;
        return `${Math.floor(mins / 1440)}d left`;
    }
    GmoUI.ttl = ttl;
    function messageTime(ms) {
        const date = new Date(ms);
        if (Number.isNaN(date.valueOf()))
            return "";
        const sameDay = new Date().toDateString() === date.toDateString();
        return sameDay
            ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
            : date.toLocaleDateString([], { month: "short", day: "numeric" });
    }
    GmoUI.messageTime = messageTime;
    function age(ms) {
        if (!Number.isFinite(ms))
            return "";
        const mins = Math.max(0, Math.floor((Date.now() - ms) / 60000));
        if (mins < 1)
            return "just now";
        if (mins < 60)
            return `${mins}m ago`;
        if (mins < 1440)
            return `${Math.floor(mins / 60)}h ago`;
        return `${Math.floor(mins / 1440)}d ago`;
    }
    GmoUI.age = age;
    function duration(seconds) {
        if (!(seconds > 0))
            return "—";
        const total = Math.round(seconds);
        return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
    }
    GmoUI.duration = duration;
    GmoUI.OTP_BACKUP_FORMAT = "gmo-temptools.otp.backup.v1";
    GmoUI.OTP_BACKUP_MAX_SECRETS = 500;
    const OTP_BACKUP_ITERATIONS = 310000;
    const OTP_BACKUP_MAX_ITERATIONS = 2000000;
    function bytesToBase64(bytes) {
        let binary = "";
        for (const byte of bytes)
            binary += String.fromCharCode(byte);
        return btoa(binary);
    }
    function base64ToBytes(value) {
        const binary = atob(value);
        const bytes = new Uint8Array(new ArrayBuffer(binary.length));
        for (let index = 0; index < binary.length; index += 1) {
            bytes[index] = binary.charCodeAt(index);
        }
        return bytes;
    }
    function cleanBackupSecret(item) {
        if (!item || typeof item !== "object")
            return null;
        const raw = item;
        if (typeof raw.label !== "string" || typeof raw.secret !== "string")
            return null;
        const label = raw.label.trim().slice(0, 80);
        const secret = raw.secret.replace(/[\s-]/g, "").toUpperCase();
        if (!label || !/^[A-Z2-7]+=*$/.test(secret) || secret.replace(/=/g, "").length < 8)
            return null;
        const digits = Number(raw.digits);
        if (digits !== 6 && digits !== 8)
            return null;
        const period = Number(raw.period);
        if (!Number.isFinite(period) || period < 15 || period > 120)
            return null;
        const algorithm = raw.algorithm === "SHA256" || raw.algorithm === "SHA512" ? raw.algorithm : "SHA1";
        return { label, secret, digits, period, algorithm };
    }
    async function deriveBackupKey(passphrase, salt, iterations) {
        const material = await crypto.subtle.importKey("raw", new TextEncoder().encode(passphrase), "PBKDF2", false, ["deriveKey"]);
        return crypto.subtle.deriveKey({ name: "PBKDF2", salt, iterations, hash: "SHA-256" }, material, { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
    }
    async function encryptOtpBackup(secrets, passphrase) {
        if (!crypto?.subtle) {
            throw new Error("This browser cannot encrypt backups (WebCrypto is unavailable).");
        }
        if (typeof passphrase !== "string" || passphrase.length < 8) {
            throw new Error("Use a backup passphrase of at least 8 characters.");
        }
        const clean = secrets
            .map(cleanBackupSecret)
            .filter((item) => Boolean(item))
            .slice(0, GmoUI.OTP_BACKUP_MAX_SECRETS);
        if (clean.length === 0) {
            throw new Error("There are no authenticator accounts to export.");
        }
        const salt = crypto.getRandomValues(new Uint8Array(16));
        const iv = crypto.getRandomValues(new Uint8Array(12));
        const key = await deriveBackupKey(passphrase, salt, OTP_BACKUP_ITERATIONS);
        const plaintext = new TextEncoder().encode(JSON.stringify({ secrets: clean }));
        const ciphertext = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, plaintext));
        return {
            format: GmoUI.OTP_BACKUP_FORMAT,
            created_at: Date.now(),
            count: clean.length,
            kdf: {
                name: "PBKDF2",
                hash: "SHA-256",
                iterations: OTP_BACKUP_ITERATIONS,
                salt: bytesToBase64(salt),
            },
            cipher: { name: "AES-GCM", iv: bytesToBase64(iv) },
            ciphertext: bytesToBase64(ciphertext),
        };
    }
    GmoUI.encryptOtpBackup = encryptOtpBackup;
    async function decryptOtpBackup(payload, passphrase) {
        if (!crypto?.subtle) {
            throw new Error("This browser cannot decrypt backups (WebCrypto is unavailable).");
        }
        if (typeof passphrase !== "string" || passphrase.length === 0) {
            throw new Error("Enter the backup passphrase.");
        }
        if (!payload || typeof payload !== "object") {
            throw new Error("That is not a GMO TempTools OTP backup.");
        }
        const file = payload;
        const kdf = file.kdf;
        const cipher = file.cipher;
        if (file.format !== GmoUI.OTP_BACKUP_FORMAT || !kdf || !cipher) {
            throw new Error("That is not a GMO TempTools OTP backup.");
        }
        const iterations = Number(kdf.iterations);
        if (kdf.name !== "PBKDF2" ||
            kdf.hash !== "SHA-256" ||
            cipher.name !== "AES-GCM" ||
            !Number.isInteger(iterations) ||
            iterations < 10000 ||
            iterations > OTP_BACKUP_MAX_ITERATIONS) {
            throw new Error("The backup uses unsupported encryption settings.");
        }
        if (typeof kdf.salt !== "string" ||
            typeof cipher.iv !== "string" ||
            typeof file.ciphertext !== "string") {
            throw new Error("The backup file is incomplete.");
        }
        let salt;
        let iv;
        let ciphertext;
        try {
            salt = base64ToBytes(kdf.salt);
            iv = base64ToBytes(cipher.iv);
            ciphertext = base64ToBytes(file.ciphertext);
        }
        catch {
            throw new Error("The backup file is damaged.");
        }
        if (salt.length < 8 || salt.length > 64 || iv.length < 12 || iv.length > 16 || ciphertext.length < 16) {
            throw new Error("The backup file is damaged.");
        }
        const key = await deriveBackupKey(passphrase, salt, iterations);
        let plaintext;
        try {
            plaintext = await crypto.subtle.decrypt({ name: "AES-GCM", iv }, key, ciphertext);
        }
        catch {
            throw new Error("Wrong passphrase, or the backup file was modified.");
        }
        let parsed;
        try {
            parsed = JSON.parse(new TextDecoder().decode(plaintext));
        }
        catch {
            throw new Error("The decrypted backup is not readable.");
        }
        if (!Array.isArray(parsed.secrets)) {
            throw new Error("The backup contains no authenticator accounts.");
        }
        const secrets = parsed.secrets
            .map(cleanBackupSecret)
            .filter((item) => Boolean(item))
            .slice(0, GmoUI.OTP_BACKUP_MAX_SECRETS);
        if (secrets.length === 0) {
            throw new Error("The backup contains no usable authenticator accounts.");
        }
        return secrets;
    }
    GmoUI.decryptOtpBackup = decryptOtpBackup;
})(GmoUI || (GmoUI = {}));
