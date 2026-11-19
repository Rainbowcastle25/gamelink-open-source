import { ChevronRight, FolderOpen, TriangleAlert } from "lucide-react";
import { useId, useState, type ReactNode } from "react";
import { cn, errorMessage, formatDate } from "../lib/format";
import { EXECUTABLE_GAMES } from "../lib/games";
import { useAppState, useStore } from "../lib/store";
import { toast } from "../lib/toast";
import { Button } from "./Button";
import { CopyButton } from "./CopyButton";
import { inputCls } from "./Field";
import { ErrorState } from "./ErrorState";
import { Skeleton } from "./Skeleton";
import { Toggle } from "./Toggle";

function Section({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  const id = useId();
  return (
    <section aria-labelledby={id} className="rounded-xl border border-line bg-surface p-6 shadow-card">
      <h2 id={id} className="text-[15px] font-semibold">
        {title}
      </h2>
      {description && <p className="mt-1 text-[13px] leading-relaxed text-muted">{description}</p>}
      <div className="mt-4 divide-y divide-line">{children}</div>
    </section>
  );
}

function Row({ title, description, children, stack }: { title: string; description?: ReactNode; children?: ReactNode; stack?: boolean }) {
  return (
    <div className={cn("py-3.5 first:pt-0 last:pb-0", stack ? "space-y-2.5" : "flex items-center justify-between gap-6")}>
      <div className="min-w-0">
        <p className="text-sm font-medium">{title}</p>
        {description && <div className="mt-0.5 text-[13px] leading-relaxed break-words text-muted">{description}</div>}
      </div>
      {children && <div className={cn(!stack && "shrink-0")}>{children}</div>}
    </div>
  );
}

/** The arguments a game is started with; saved when the field loses focus. */
function LaunchArgsField({ game, label, hint, value }: { game: string; label: string; hint: string; value: string }) {
  const store = useStore();
  const id = useId();
  const [text, setText] = useState(value);
  return (
    <div className="mt-2.5">
      <label htmlFor={id} className="mb-1 block text-xs font-medium text-muted">
        Connect arguments
      </label>
      <input
        id={id}
        aria-label={`Connect arguments for ${label}`}
        aria-describedby={`${id}-hint`}
        className={cn(inputCls, "font-mono text-xs")}
        value={text}
        placeholder="None"
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => void store.setLaunchArgs(game, text.trim()).then((ok) => !ok && setText(value))}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
        }}
      />
      <p id={`${id}-hint`} className="mt-1 text-xs leading-relaxed text-faint">
        {hint}
      </p>
    </div>
  );
}

function Mono({ children }: { children: ReactNode }) {
  return <code className="font-mono text-xs break-all text-fg/90">{children}</code>;
}

function PrismFolderField({ value }: { value: string }) {
  const store = useStore();
  const id = useId();
  const [text, setText] = useState(value);
  return (
    <>
      <label htmlFor={id} className="sr-only">Prism instances folder</label>
      <input
        id={id}
        className={inputCls}
        value={text}
        placeholder="Full path to the instances folder"
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          const folder = text.trim();
          setText(folder);
          if (folder !== value) {
            void store.updateSettings({ prismInstancesDir: folder }).then((ok) => {
              if (!ok) setText(value);
            });
          }
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
        }}
      />
    </>
  );
}

export function SettingsPanel() {
  const { ready, settings, identity, paths, version } = useAppState();
  const store = useStore();
  const [advanced, setAdvanced] = useState(false);
  const advId = useId();

  if (!ready) {
    return (
      <div className="space-y-4" role="status" aria-label="Loading settings">
        <Skeleton className="h-40" />
        <Skeleton className="h-40" />
      </div>
    );
  }
  if (!settings) {
    return <ErrorState title="Could not load settings" message="GameLink could not read its settings." onRetry={() => void store.retryLoad()} />;
  }

  const browse = async (game: string, label: string) => {
    try {
      const picked = await store.backend.pickExecutable(`Choose the program for ${label}`);
      if (picked) await store.setExecutable(game, picked);
    } catch (e) {
      toast.error(errorMessage(e));
    }
  };

  return (
    <div className="space-y-5">
      <Section title="Device identity" description="This device's identity is created once and stays here. The private key never leaves this device and is never shown.">
        <Row title="Public key" stack description="Safe to share. The server owner needs it to approve this device.">
          <div className="rounded-lg border border-line-strong bg-black/25 px-3.5 py-2.5">
            <Mono>{identity?.publicKey ?? "Unavailable"}</Mono>
          </div>
          {identity && <CopyButton text={identity.publicKey} label="Copy Public Key" size="sm" />}
        </Row>
        {identity && (
          <>
            <Row title="Created" description={formatDate(identity.created)} />
            <Row title="Storage folder" description={<Mono>{identity.configDir}</Mono>} />
            <Row title="Protection" description={identity.protection} />
          </>
        )}
      </Section>

      <Section title="Storage locations" description="Everything GameLink keeps lives in one folder on this PC.">
        {paths && (
          <>
            <Row title="Settings and servers" description={<Mono>{paths.configFile}</Mono>} />
            <Row title="Device identity file" description={<Mono>{paths.identityFile}</Mono>} />
            <Row title="Activity history" description={<Mono>{paths.activityFile}</Mono>} />
            <Row title="Log file" description={<Mono>{paths.logFile}</Mono>} />
          </>
        )}
        <Row title="Open the storage folder">
          <Button size="sm" icon={<FolderOpen className="size-4" aria-hidden />} onClick={() => void store.backend.openConfigFolder().catch((e: unknown) => toast.error(errorMessage(e)))}>
            Open folder
          </Button>
        </Row>
      </Section>

      <Section title="Connection preferences">
        <Row title="Reconnect automatically" description="If a connection drops, try to restore it without you clicking Join again.">
          <Toggle label="Reconnect automatically" checked={settings.autoReconnect} onChange={(v) => void store.updateSettings({ autoReconnect: v })} />
        </Row>
        <Row title="Check servers on startup" description="Quickly see which servers answer when GameLink opens.">
          <Toggle label="Check servers on startup" checked={settings.checkOnStartup} onChange={(v) => void store.updateSettings({ checkOnStartup: v })} />
        </Row>
        <Row title="Notify me when servers come online" description="Checks saved servers about every two minutes while GameLink is open. Each check reveals to the host’s agent that you are online.">
          <Toggle label="Notify me when servers come online" checked={settings.presenceNotifications} onChange={(v) => void store.updateSettings({ presenceNotifications: v })} />
        </Row>
        <div className="pt-3.5">
          <button
            type="button"
            aria-expanded={advanced}
            aria-controls={advId}
            onClick={() => setAdvanced((a) => !a)}
            className="flex items-center gap-1.5 rounded text-[13px] font-medium text-muted transition-colors duration-150 hover:text-fg"
          >
            <ChevronRight className={cn("size-4 transition-transform duration-200", advanced && "rotate-90")} aria-hidden />
            Advanced
          </button>
          {advanced && (
            <div id={advId} className="mt-3 rounded-lg border border-line bg-black/15 p-4">
              <div className="flex items-center justify-between gap-6">
                <div className="min-w-0">
                  <p className="text-sm font-medium">Allow relays on private networks</p>
                  <p className="mt-0.5 flex items-start gap-2 text-[13px] leading-relaxed text-warn">
                    <TriangleAlert className="mt-0.5 size-4 shrink-0" aria-hidden />
                    Only for self-hosted relays on your own LAN. This weakens the checks GameLink makes on invitations. Leave it off unless the server owner told you to turn it on.
                  </p>
                </div>
                <Toggle label="Allow relays on private networks" checked={settings.allowLocalRelay} onChange={(v) => void store.updateSettings({ allowLocalRelay: v })} />
              </div>
            </div>
          )}
        </div>
      </Section>

      <Section title="Minecraft Java">
        <Row title="Add GameLink servers to Minecraft's multiplayer list" description="Find your servers by name under Multiplayer after joining in GameLink. Close Minecraft before changing its list.">
          <Toggle label="Add GameLink servers to Minecraft's multiplayer list" checked={settings.minecraftServerList} onChange={(v) => void store.updateSettings({ minecraftServerList: v })} />
        </Row>
        {settings.minecraftServerList && (
          <Row title="Prism instances folder" stack description="Optional. Also sync the multiplayer lists in the Prism instances in this folder.">
            <PrismFolderField key={settings.prismInstancesDir} value={settings.prismInstancesDir} />
          </Row>
        )}
        {settings.minecraftSyncError && (
          <p className="pt-3.5 text-[13px] text-warn" role="alert">Minecraft list sync failed. {settings.minecraftSyncError}</p>
        )}
      </Section>

      <Section
        title="Local game executables"
        description="Optional. Set a game's program and its server card gets a Play button: GameLink connects, then starts the game and points it at the server. Networking never depends on it."
      >
        {EXECUTABLE_GAMES.map((g) => {
          const path = settings.gameExecutables?.[g.id];
          return (
            <div key={g.id} className="py-3.5 first:pt-0 last:pb-0">
              <div className="flex items-center justify-between gap-6">
                <div className="min-w-0">
                  <p className="text-sm font-medium">{g.label}</p>
                  <div className="mt-0.5 text-[13px] leading-relaxed break-words text-muted">{path ? <Mono>{path}</Mono> : (g.noProgram ?? "Not set")}</div>
                </div>
                <div className="flex shrink-0 gap-2">
                  <Button size="sm" aria-label={`Browse for ${g.label}`} onClick={() => void browse(g.id, g.label)}>
                    Browse
                  </Button>
                  {path && (
                    <Button size="sm" variant="ghost" aria-label={`Clear ${g.label}`} onClick={() => void store.setExecutable(g.id, "")}>
                      Clear
                    </Button>
                  )}
                </div>
              </div>
              {(path || !g.noProgram) && (
                <LaunchArgsField
                  key={settings.gameLaunchArgs?.[g.id] ?? ""}
                  game={g.id}
                  label={g.label}
                  hint={g.argsHint}
                  value={settings.gameLaunchArgs?.[g.id] ?? ""}
                />
              )}
            </div>
          );
        })}
      </Section>

      <Section title="Logging and diagnostics">
        <Row title="Verbose network logging" description={`Secrets are redacted. Written to ${paths?.logFile ?? "the log file"}.`}>
          <Toggle label="Verbose network logging" checked={settings.verboseLogging} onChange={(v) => void store.updateSettings({ verboseLogging: v })} />
        </Row>
        <Row title="Activity history" description="The short list of events shown on the Dashboard.">
          <Button size="sm" onClick={() => void store.clearActivity()}>
            Clear activity history
          </Button>
        </Row>
      </Section>

      <Section title="Demo Mode">
        <Row
          title="Show sample servers"
          description="Adds clearly labelled sample servers so you can try the interface. They are not real and do not connect to anything."
        >
          <Toggle label="Demo Mode" checked={settings.demoMode} onChange={(v) => void store.updateSettings({ demoMode: v })} />
        </Row>
      </Section>

      <Section title="About">
        <Row title="Version" description={version ? `${version.app} ${version.version}` : "Unknown"} />
        <Row
          title="Check for updates"
          description={
            settings.canCheckForUpdates
              ? "Check for signed release notices once a day while GameLink is open. You install updates yourself."
              : "This build can't check for updates because it has no release verification key."
          }
        >
          <Toggle
            label="Check for updates"
            checked={settings.checkForUpdates && settings.canCheckForUpdates}
            disabled={!settings.canCheckForUpdates}
            onChange={(v) => void store.updateSettings({ checkForUpdates: v })}
          />
        </Row>
        <Row title="Transport" description={version?.transport ?? "Unknown"} />
        <Row
          title="Alpha software"
          description="Encrypted tunnels do not protect you from vulnerabilities in the game server itself. GameLink is not security-audited."
        />
        <Row title="License" description="GPL-3.0" />
      </Section>
    </div>
  );
}
