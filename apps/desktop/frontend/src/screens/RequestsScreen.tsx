import { Header } from "../components/Header";
import { CopyButton } from "../components/CopyButton";
import { ErrorState } from "../components/ErrorState";
import { RequestInstructions } from "../components/RequestInstructions";
import { Skeleton } from "../components/Skeleton";
import { useNav } from "../lib/nav";
import { useAppState, useStore } from "../lib/store";

export function RequestsScreen() {
  const { ready, identity } = useAppState();
  const store = useStore();
  const { go } = useNav();

  return (
    <>
      <Header title="Requests" subtitle="How to ask a server owner for access" />
      <div className="space-y-5">
        <section aria-labelledby="your-device" className="rounded-xl border border-line bg-surface p-6 shadow-card">
          <h2 id="your-device" className="text-base font-semibold">
            Your device
          </h2>
          <p className="mt-1 text-[13px] text-muted">This is the public key that identifies this device. It is safe to share; the matching private key never leaves this PC.</p>
          {!ready ? (
            <div role="status" aria-label="Loading device key" className="mt-4 space-y-3">
              <Skeleton className="h-16" />
            </div>
          ) : identity ? (
            <div className="mt-4 space-y-3">
              <div className="rounded-lg border border-line-strong bg-black/30 px-4 py-3">
                <code className="block font-mono text-sm leading-relaxed break-all select-all" aria-label="Public key">
                  {identity.publicKey}
                </code>
              </div>
              <CopyButton text={identity.publicKey} label="Copy Public Key" variant="primary" />
            </div>
          ) : (
            <div className="mt-4">
              <ErrorState variant="inline" title="Could not read this device's key" onRetry={() => void store.retryLoad()} />
            </div>
          )}
        </section>

        {identity && <RequestInstructions publicKey={identity.publicKey} onImport={() => go("servers", { openImport: true })} />}
      </div>
    </>
  );
}
