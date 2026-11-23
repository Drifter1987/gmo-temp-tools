/**
 * Shared browser helpers for the three GMO temporary-services frontends.
 *
 * Compiled to /ui.js and loaded before each app's /app.js. This is a global
 * namespace on purpose: the apps ship standalone scripts with no bundler.
 */
namespace GmoUI {
  export function $<T extends Element = Element>(
    selector: string,
    root: ParentNode = document,
  ): T | null {
    return root.querySelector<T>(selector);
  }

  export function esc(value: unknown): string {
    return String(value ?? "").replace(
      /[&<>"']/g,
      (character) =>
        ({
          "&": "&amp;",
          "<": "&lt;",
          ">": "&gt;",
          '"': "&quot;",
          "'": "&#39;",
        })[character] ?? character,
    );
  }

  let toastTimer: number | undefined;

  export function toast(text: string, timeout = 1600): void {
    let element = $<HTMLElement>(".toast");
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

  export async function copy(text: string, timeout = 1600): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
      toast("Copied", timeout);
    } catch {
      const textarea = document.createElement("textarea");
      textarea.value = text;
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand("copy");
      textarea.remove();
      toast("Copied", timeout);
    }
  }

  export async function readJson<T>(response: Response): Promise<T | null> {
    try {
      return (await response.json()) as T;
    } catch {
      return null;
    }
  }

  export function updateTtls(format: (remainingMs: number) => string): void {
    const now = Date.now();
    document.querySelectorAll<HTMLElement>("[data-ttl]").forEach((element) => {
      const holder = element.closest<HTMLElement>("[data-expires]") ?? element;
      const expires = Number(holder.dataset.expires ?? element.dataset.expires ?? 0);
      if (!expires) return;
      element.textContent = format(expires - now);
    });
  }

  export function ttl(remainingMs: number, compact = false): string {
    if (!Number.isFinite(remainingMs)) return "";
    const mins = Math.max(0, Math.ceil(remainingMs / 60_000));
    if (mins <= 0) return "expired";
    if (mins < 60) return compact ? `${mins}m left` : `${mins} min left`;
    if (mins < 1440) return `${Math.floor(mins / 60)}h left`;
    return `${Math.floor(mins / 1440)}d left`;
  }

  export function messageTime(ms: number): string {
    const date = new Date(ms);
    if (Number.isNaN(date.valueOf())) return "";
    const sameDay = new Date().toDateString() === date.toDateString();
    return sameDay
      ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
      : date.toLocaleDateString([], { month: "short", day: "numeric" });
  }

  export function age(ms: number): string {
    if (!Number.isFinite(ms)) return "";
    const mins = Math.max(0, Math.floor((Date.now() - ms) / 60_000));
    if (mins < 1) return "just now";
    if (mins < 60) return `${mins}m ago`;
    if (mins < 1440) return `${Math.floor(mins / 60)}h ago`;
    return `${Math.floor(mins / 1440)}d ago`;
  }

  export function duration(seconds: number): string {
    if (!(seconds > 0)) return "—";
    const total = Math.round(seconds);
    return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
  }
}
