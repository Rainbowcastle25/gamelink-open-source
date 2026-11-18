import { CircleCheck, LoaderCircle, ShieldCheck, TriangleAlert } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { errorMessage, parseOptionalPort, sentence } from "../lib/format";
import { protoLabel } from "../lib/games";
import { useStore } from "../lib/store";
import { toast } from "../lib/toast";
import type { InvitePreview } from "../lib/types";
import { Button } from "./Button";
import { Field, inputCls } from "./Field";
import { GameThumb } from "./GameThumb";
import { Modal } from "./Modal";

export function InvitePreviewCard({ preview }: { preview: InvitePreview }) {
  const warnings = preview.warnings ?? [];
  const relays = (preview.relayHosts ?? []).join(", ");
  const custom = !preview.defaultRelay;
  return (
    <div className="rounded-xl border border-line-strong bg-black/20 p-4" aria-label="Invitation preview" role="group">
      <div className="flex items-center gap-3.5">
        <GameThumb game={preview.game} className="size-11" />
        <div className="min-w-0">
          <p className="truncate text-[15px] font-semibold">{preview.name}</p>
          <p className="truncate text-[13px] text-muted">
            {preview.gameName} · {protoLabel(preview.proto || "tcp")} port {preview.port}
          </p>
        </div>
      </div>
      {preview.note && <p className="mt-3 border-l-2 border-teal/50 pl-3 text-[13px] leading-relaxed text-fg/85">{preview.note}</p>}
      <p className="mt-3 truncate text-xs text-muted">
        Relay: <span className="font-mono text-fg/85">{relays || "unknown"}</span>
      </p>
      {preview.defaultRelay && (
        <p className="mt-2 flex items-start gap-2 text-[13px] text-muted">
          <ShieldCheck className="mt-0.5 size-4 shrink-0 text-ok" aria-hidden />
          This server uses the public Tailcat relays, which only pass along encrypted traffic.
        </p>
      )}
      {(custom || warnings.length > 0) && (
        <ul className="mt-3 space-y-2" aria-label="Warnings">
          {(warnings.length > 0 ? warnings : ["This server uses a custom relay instead of the public Tailcat relays. Only continue if you trust whoever sent you this invitation."]).map((w) => (
            <li key={w} className="flex items-start gap-2.5 rounded-lg border border-warn/30 bg-warn/8 px-3 py-2.5 text-[13px] leading-snug text-[#f6d38a]">
              <TriangleAlert className="mt-0.5 size-4 shrink-0 text-warn" aria-hidden />
              <span>{w}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function ImportInviteModal({ onClose }: { onClose: () => void }) {
  const store = useStore();
  const [text, setText] = useState("");
  const [portText, setPortText] = useState("");
  const [preview, setPreview] = useState<InvitePreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const seq = useRef(0);

  // Live, debounced validation by the backend. The sequence number drops
  // answers that arrive out of order.
  useEffect(() => {
    const invitation = text.trim();
    const mine = ++seq.current;
    setSubmitError(null);
    if (!invitation) {
      setPreview(null);
      setPreviewError(null);
      setChecking(false);
      return;
    }
    setChecking(true);
    const handle = setTimeout(() => {
      store.backend
        .previewInvite(invitation)
        .then((p) => {
          if (seq.current !== mine) return;
          setPreview(p);
          setPreviewError(null);
        })
        .catch((e: unknown) => {
          if (seq.current !== mine) return;
          setPreview(null);
          setPreviewError(sentence(errorMessage(e)));
        })
        .finally(() => {
          if (seq.current === mine) setChecking(false);
        });
    }, 350);
    return () => clearTimeout(handle);
  }, [text, store]);

  const port = parseOptionalPort(portText);
  const portError = port === null ? "Enter a port between 1 and 65535, or leave this empty for automatic." : null;
  const canSubmit = !!preview && !checking && port !== null && !submitting;

  const submit = async () => {
    if (!preview || port === null) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      const added = await store.importInvite(text.trim(), port);
      toast.success(`Added “${added.name}”`);
      onClose();
    } catch (e) {
      setSubmitError(sentence(errorMessage(e)));
      setSubmitting(false);
    }
  };

  return (
    <Modal
      title="Import invitation"
      onClose={onClose}
      width="max-w-2xl"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void submit()} disabled={!canSubmit} loading={submitting}>
            Add server
          </Button>
        </>
      }
    >
      <form
        className="space-y-5"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <Field label="Invitation" hint="Paste the invitation your server owner sent you. It is a secret, so only paste invitations from people you trust." error={previewError}>
          {({ id, describedBy }) => (
            <textarea
              id={id}
              data-autofocus
              aria-describedby={describedBy}
              aria-invalid={!!previewError}
              rows={4}
              value={text}
              onChange={(e) => setText(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              placeholder="glink1.…"
              className={`${inputCls} resize-none font-mono text-xs leading-relaxed`}
            />
          )}
        </Field>

        {checking && (
          <p className="flex items-center gap-2 text-[13px] text-muted" role="status">
            <LoaderCircle className="size-4 animate-spin-slow text-accent" aria-hidden /> Checking the invitation...
          </p>
        )}
        {preview && !checking && (
          <div className="space-y-1.5">
            <p className="flex items-center gap-1.5 text-[13px] text-ok">
              <CircleCheck className="size-4" aria-hidden /> Invitation looks valid
            </p>
            <InvitePreviewCard preview={preview} />
          </div>
        )}

        <Field label="Local port (leave empty for automatic)" error={portError} hint="GameLink assigns a stable port when you leave this blank.">
          {({ id, describedBy }) => (
            <input
              id={id}
              aria-describedby={describedBy}
              aria-invalid={!!portError}
              inputMode="numeric"
              value={portText}
              onChange={(e) => setPortText(e.target.value)}
              placeholder="Automatic"
              className={`${inputCls} max-w-48`}
            />
          )}
        </Field>

        {submitError && (
          <p role="alert" className="rounded-lg border border-danger/30 bg-danger/8 px-3.5 py-2.5 text-[13px] text-danger">
            {submitError}
          </p>
        )}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}
