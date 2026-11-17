import { useEffect, useState } from "react";
import { errorMessage } from "../lib/format";
import { useStore } from "../lib/store";
import type { ServerView } from "../lib/types";
import { Button } from "./Button";
import { CopyButton } from "./CopyButton";
import { ErrorState } from "./ErrorState";
import { Modal } from "./Modal";
import { Skeleton } from "./Skeleton";

export function DiagnosticsModal({ server, onClose }: { server: ServerView; onClose: () => void }) {
  const store = useStore();
  const [report, setReport] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    store.backend
      .diagnostics(server.id)
      .then((r) => {
        if (!cancelled) {
          setReport(r);
          setError(null);
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [store, server.id, attempt]);

  return (
    <Modal
      title={`Diagnostics: ${server.name}`}
      onClose={onClose}
      width="max-w-2xl"
      footer={
        <>
          {report && <CopyButton text={report} label="Copy report" />}
          <Button variant="primary" onClick={onClose}>
            Close
          </Button>
        </>
      }
    >
      {error ? (
        <ErrorState
          variant="inline"
          title="Could not create the report"
          message={error}
          onRetry={() => {
            setError(null);
            setAttempt((a) => a + 1);
          }}
        />
      ) : report === null ? (
        <div className="space-y-2" role="status" aria-label="Loading report">
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
          <Skeleton className="h-4 w-3/4" />
        </div>
      ) : (
        <>
          <p className="mb-3 text-[13px] text-muted">Share this with whoever helps you troubleshoot. It contains no secrets.</p>
          <pre
            tabIndex={0}
            aria-label="Diagnostics report"
            className="max-h-[50vh] overflow-auto rounded-lg border border-line bg-black/30 p-4 font-mono text-xs leading-relaxed whitespace-pre-wrap text-fg/90 [overflow-wrap:anywhere]"
          >
            {report}
          </pre>
        </>
      )}
    </Modal>
  );
}
