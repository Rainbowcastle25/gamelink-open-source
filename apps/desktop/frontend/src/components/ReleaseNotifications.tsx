import { ArrowUpRight, Info, ShieldAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { cn, errorMessage } from "../lib/format";
import { useAppState, useStore } from "../lib/store";
import { toast } from "../lib/toast";
import type { ReleaseNotice, UpdateProgress } from "../lib/types";
import { Button } from "./Button";

const CHECK_INTERVAL = 4 * 60 * 60 * 1000;

export function ReleaseNotifications() {
  const { settings } = useAppState();
  return settings?.checkForUpdates && settings.canCheckForUpdates && <EnabledNotifications />;
}

function EnabledNotifications() {
  const { backend } = useStore();
  const [notice, setNotice] = useState<ReleaseNotice | null>(null);
  const [retry, setRetry] = useState(0);
  const [checkError, setCheckError] = useState<string | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | { stage: "restarting" } | null>(null);
  const [installed, setInstalled] = useState(false);
  const version = notice?.version;

  useEffect(() => {
    let active = true;
    async function refresh() {
      try {
        const result = await backend.checkRelease();
        if (!active) return;
        setNotice(result);
        setCheckError(result?.installerError ?? null);
      } catch (error) {
        if (!active) return;
        setNotice((current) => current && { ...current, canInstall: false });
        setCheckError(errorMessage(error));
      }
    }
    void refresh();
    const timer = setInterval(() => void refresh(), CHECK_INTERVAL);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [backend, retry]);

  async function install() {
    if (progress || (!version && !installed)) return;
    setInstallError(null);
    try {
      if (!installed && version) {
        setProgress({ stage: "downloading", downloaded: 0, total: null });
        await backend.installUpdate(version, setProgress);
        setInstalled(true);
      }
      setProgress({ stage: "restarting" });
      await backend.relaunch();
    } catch (error) {
      setProgress(null);
      setInstallError(errorMessage(error));
    }
  }

  if (!notice && !checkError && !progress && !installed) return null;
  const security = notice?.severity === "security";
  const Icon = security ? ShieldAlert : Info;

  return (
    <div className="mx-auto w-full max-w-[1180px] shrink-0 px-8 pt-5">
      <aside
        aria-label="GameLink updates"
        className={cn(
          "flex items-start gap-3 rounded-lg border px-4 py-3 text-sm",
          security ? "border-warn/60 bg-warn/10" : "border-line bg-surface",
        )}
      >
        <Icon aria-hidden className={cn("mt-0.5 size-4 shrink-0", security ? "text-warn" : "text-muted")} />
        <div className="min-w-0 flex-1">
          {notice && (
            <>
              <p role="status" className={security ? "font-semibold text-warn" : "font-medium"}>
                {security ? "Security update available" : "Update available"} · GameLink {notice.version}
              </p>
              {notice.summary && <p className="mt-1 break-words text-muted">{notice.summary}</p>}
            </>
          )}
          {progress && <InstallProgress progress={progress} />}
          {(installError || checkError) && <p role="alert" className="mt-1 text-warn">{installError || checkError}</p>}
        </div>
        <div className="ml-auto flex shrink-0 items-center gap-3">
          {notice && (
            <a
              href={notice.releaseNotesUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1 text-fg underline underline-offset-4"
              onClick={(event) => {
                event.preventDefault();
                void backend.openReleaseNotes().catch((error) => toast.error(errorMessage(error)));
              }}
            >
              Release notes <ArrowUpRight aria-hidden className="size-3.5" />
            </a>
          )}
          {notice?.canInstall || installed ? (
            <Button variant="primary" disabled={!!progress} onClick={() => void install()}>
              {progress ? "Updating…" : installed ? "Restart GameLink" : installError ? "Retry update" : "Update"}
            </Button>
          ) : checkError && !progress && (
            <Button onClick={() => {
              setCheckError(null);
              setRetry((count) => count + 1);
            }}>
              Retry check
            </Button>
          )}
        </div>
      </aside>
    </div>
  );
}

function InstallProgress({ progress }: { progress: UpdateProgress | { stage: "restarting" } }) {
  if (progress.stage === "downloading") {
    const { downloaded, total } = progress;
    const percent = total ? Math.min(100, Math.floor(downloaded * 100 / total)) : undefined;
    return (
      <div className="mt-2 flex items-center gap-2 text-muted">
        <progress aria-label="Downloading update" className="h-2 w-32 accent-accent" max={100} value={percent} />
        <span>Downloading update{percent === undefined ? "…" : ` · ${percent}%`}</span>
      </div>
    );
  }
  return (
    <p role="status" className="mt-1 text-muted">
      {progress.stage === "verifying" ? "Verifying installer…" :
        progress.stage === "installing" ? "Installing update…" : "Restarting GameLink…"}
    </p>
  );
}
