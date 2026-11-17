import { FileInput } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "./Button";
import { CopyButton } from "./CopyButton";

/** The exact command the server owner runs, with this device's key filled in. */
export function approveCommand(publicKey: string): string {
  return `gamelink-agent devices approve --game <game-id> --key ${publicKey} --label "<your name>"`;
}

function Step({ n, title, children }: { n: number; title: string; children?: ReactNode }) {
  return (
    <li className="flex gap-4">
      <span aria-hidden className="mt-0.5 grid size-7 shrink-0 place-items-center rounded-full border border-teal/40 bg-teal/12 text-[13px] font-semibold text-accent">
        {n}
      </span>
      <div className="min-w-0 flex-1 pb-1">
        <h3 className="text-sm font-semibold">{title}</h3>
        {children && <div className="mt-1 text-[13px] leading-relaxed text-muted">{children}</div>}
      </div>
    </li>
  );
}

interface Props {
  publicKey: string;
  onImport?: () => void;
}

export function RequestInstructions({ publicKey, onImport }: Props) {
  const command = approveCommand(publicKey);
  return (
    <section aria-labelledby="how-to-join" className="rounded-xl border border-line bg-surface p-6 shadow-card">
      <h2 id="how-to-join" className="text-base font-semibold">
        How to get access to a server
      </h2>
      <ol className="mt-5 space-y-5">
        <Step n={1} title="Copy your public key">
          It is shown above. It identifies this device and is safe to share.
        </Step>
        <Step n={2} title="Send it to the server owner">
          Use any private channel: a message, an email, a chat.
        </Step>
        <Step n={3} title="The owner approves it for a specific game">
          They run this command on the machine that hosts the game, with your key already filled in:
          <div className="mt-3 rounded-lg border border-line-strong bg-black/30">
            <pre className="overflow-x-auto px-4 py-3 font-mono text-xs leading-relaxed whitespace-pre-wrap text-fg/90 [overflow-wrap:anywhere]" aria-label="Approve command">
              {command}
            </pre>
            <div className="flex justify-end border-t border-line px-2 py-1.5">
              <CopyButton text={command} label="Copy Command" variant="ghost" size="sm" />
            </div>
          </div>
        </Step>
        <Step n={4} title="The owner sends you an invitation privately">
          An invitation is a secret. Treat it like a password and do not post it anywhere public.
        </Step>
        <Step n={5} title="Import it on the Servers screen, then click Join">
          {onImport && (
            <Button variant="primary" size="sm" className="mt-2" onClick={onImport} icon={<FileInput className="size-4" aria-hidden />}>
              Import invitation
            </Button>
          )}
        </Step>
      </ol>
      <p className="mt-6 rounded-lg border border-line bg-white/3 px-4 py-3 text-[13px] leading-relaxed text-muted">
        GameLink does not send or sync requests through any cloud service in this alpha. Nothing leaves your device until you share your public key yourself.
      </p>
    </section>
  );
}
