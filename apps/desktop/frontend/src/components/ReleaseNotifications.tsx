import { ArrowUpRight, Info, ShieldAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { cn, errorMessage } from "../lib/format";
import { useAppState, useStore } from "../lib/store";
import { toast } from "../lib/toast";
import type { ReleaseNotice, SettingsView } from "../lib/types";

export function ReleaseNotifications() {
  const { settings } = useAppState();
  return settings?.checkForUpdates && settings.canCheckForUpdates && <EnabledNotifications settings={settings} />;
}

function EnabledNotifications({ settings }: { settings: SettingsView }) {
  const { backend } = useStore();
  const [notice, setNotice] = useState<ReleaseNotice | null>(null);

  useEffect(() => {
    let active = true;
    async function refresh() {
      const result = await backend.checkRelease().catch(() => null);
      if (active) setNotice(result);
    }
    void refresh();
    // The native host enforces the 24-hour interval across restarts.
    const timer = setInterval(() => void refresh(), 15 * 60 * 1000);
    return () => {
      active = false;
      clearInterval(timer);
    };
    // Recheck after the optimistic setting is saved by the native host.
  }, [backend, settings]);

  if (!notice) return null;
  const security = notice.severity === "security";
  const Icon = security ? ShieldAlert : Info;
  return (
    <aside
      role="status"
      className={cn(
        "mb-5 flex items-center gap-3 rounded-lg border px-4 py-3 text-sm",
        security ? "border-warn/60 bg-warn/10" : "border-line bg-surface",
      )}
    >
      <Icon aria-hidden className={cn("size-4 shrink-0", security ? "text-warn" : "text-muted")} />
      <span className={security ? "font-semibold text-warn" : "font-medium"}>
        {security ? "Security update available" : "Update available"} · GameLink {notice.version}
      </span>
      <a
        href={notice.releaseNotesUrl}
        target="_blank"
        rel="noopener noreferrer"
        className="ml-auto inline-flex shrink-0 items-center gap-1 text-sm text-fg underline underline-offset-4"
        onClick={(event) => {
          event.preventDefault();
          void backend.openReleaseNotes().catch((error) => toast.error(errorMessage(error)));
        }}
      >
        Release notes <ArrowUpRight aria-hidden className="size-3.5" />
      </a>
    </aside>
  );
}
