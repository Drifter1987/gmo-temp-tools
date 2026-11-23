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
})(GmoUI || (GmoUI = {}));
