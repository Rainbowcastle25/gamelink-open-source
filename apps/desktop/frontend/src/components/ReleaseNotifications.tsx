import { ArrowUpRight, Info, ShieldAlert } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { cn, errorMessage } from "../lib/format";
import { useAppState, useStore } from "../lib/store";
import { toast } from "../lib/toast";
import type { AgentUpdateProgress, ReleaseNotice, UpdateProgress } from "../lib/types";
import { Button } from "./Button";

const CHECK_INTERVAL = 4 * 60 * 60 * 1000;
const MANUAL_AGENT_UPDATE = "Update the GameLink agent on your server with the setup guide or your package manager.";
const AGENT_PROGRESS: Record<AgentUpdateProgress, string> = {
  preparing: "Preparing the server update…",
  installing: "Installing the server update…",
  restarting: "Waiting for your server agent to restart…",
  checkingHealth: "Checking server agent health…",
};

type Progress = UpdateProgress | { stage: "server"; phase: AgentUpdateProgress } | { stage: "restarting" };

export function ReleaseNotifications() {
  const { settings } = useAppState();
  return settings?.checkForUpdates && settings.canCheckForUpdates && <EnabledNotifications />;
}

function EnabledNotifications() {
  const { backend } = useStore();
  const { hostConnection } = useAppState();
  const [notice, setNotice] = useState<ReleaseNotice | null>(null);
  const [retry, setRetry] = useState(0);
  const [checkError, setCheckError] = useState<string | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);
  const [serverError, setServerError] = useState<string | null>(null);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [installed, setInstalled] = useState(false);
  const busy = useRef(false);
  const version = notice?.version;
  const linkedAgent = notice?.canInstall && !installed ? notice.linkedAgent : null;
  const updateAll = linkedAgent?.kind === "available";

  useEffect(() => {
    let active = true;
    async function refresh() {
      if (busy.current) return;
      try {
        const result = await backend.checkRelease();
        if (!active) return;
        setNotice(result);
        setCheckError(result?.installerError ?? null);
        setServerError(null);
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
  }, [backend, hostConnection, retry]);

  async function install(includeServer = false) {
    if (busy.current || (!version && !installed)) return;
    busy.current = true;
    setInstallError(null);
    setServerError(null);
    try {
      if (!installed && version) {
        if (includeServer) {
          let receivingServerProgress = true;
          setProgress({ stage: "server", phase: "preparing" });
          try {
            await backend.updateLinkedAgent(version, (phase) => {
              if (receivingServerProgress) setProgress({ stage: "server", phase });
            });
            setNotice((current) => current?.version === version ? { ...current, linkedAgent: null } : current);
          } catch (error) {
            if (error && typeof error === "object" && "kind" in error && error.kind === "manual") {
              setNotice((current) => current?.version === version
                ? { ...current, linkedAgent: { kind: "manual" } }
                : current);
            } else {
              setServerError(errorMessage(error));
            }
            setProgress(null);
            busy.current = false;
            return;
          } finally {
            receivingServerProgress = false;
          }
        }
        setProgress({ stage: "downloading", downloaded: 0, total: null });
        await backend.installUpdate(version, setProgress);
        setInstalled(true);
      }
      setProgress({ stage: "restarting" });
      await backend.relaunch();
    } catch (error) {
      setProgress(null);
      setInstallError(errorMessage(error));
      busy.current = false;
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
              {linkedAgent?.kind === "available" && <p className="mt-1 break-words text-muted">
                Update this PC and your server {linkedAgent.nodeName}.
              </p>}
              {linkedAgent?.kind === "manual" && <p className="mt-1 text-muted">{MANUAL_AGENT_UPDATE}</p>}
              {linkedAgent?.kind === "unreachable" && <p className="mt-1 text-muted">
                Could not check the GameLink agent on your server.
              </p>}
            </>
          )}
          {progress && <InstallProgress progress={progress} />}
          {(serverError || installError || checkError) && <p role="alert" className="mt-1 break-words text-warn">
            {serverError || installError || checkError}
          </p>}
        </div>
        <div className="ml-auto flex max-w-[60%] shrink-0 flex-wrap items-center justify-end gap-3">
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
          {updateAll ? (
            <>
              <Button disabled={!!progress} onClick={() => void install()}>Update this app only</Button>
              <Button variant="primary" disabled={!!progress} onClick={() => void install(true)}>
                {progress?.stage === "server" ? "Updating server…" : serverError ? "Retry" : "Update all"}
              </Button>
            </>
          ) : notice?.canInstall || installed ? (
            <Button variant="primary" disabled={!!progress} onClick={() => void install()}>
              {progress ? "Updating…" : installed ? "Restart GameLink" :
                installError ? "Retry update" : linkedAgent ? "Update this app only" : "Update"}
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

function InstallProgress({ progress }: { progress: Progress }) {
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
      {progress.stage === "server" ? AGENT_PROGRESS[progress.phase] :
        progress.stage === "verifying" ? "Verifying installer…" :
        progress.stage === "installing" ? "Installing update…" : "Restarting GameLink…"}
    </p>
  );
}
