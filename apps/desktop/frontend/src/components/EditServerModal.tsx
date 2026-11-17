import { TriangleAlert } from "lucide-react";
import { useState } from "react";
import { errorMessage, parseOptionalPort, sentence } from "../lib/format";
import { useStore } from "../lib/store";
import { toast } from "../lib/toast";
import type { ServerView } from "../lib/types";
import { Button } from "./Button";
import { Field, inputCls } from "./Field";
import { Modal } from "./Modal";

export function EditServerModal({ server, onClose }: { server: ServerView; onClose: () => void }) {
  const store = useStore();
  const [name, setName] = useState(server.name);
  const [description, setDescription] = useState(server.description);
  const [localText, setLocalText] = useState(server.localPort ? String(server.localPort) : "");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const live = server.connection.state !== "disconnected";
  const localPort = parseOptionalPort(localText);
  const nameError = name.trim() ? null : "Give the server a name.";
  const portError = localPort === null ? "Enter a port between 1 and 65535, or leave this empty for automatic." : null;
  const valid = !nameError && !portError;

  const submit = async () => {
    if (!valid || localPort === null) return;
    setSubmitting(true);
    setError(null);
    try {
      await store.updateServer(server.id, name.trim(), description.trim(), localPort);
      toast.success("Saved");
      onClose();
    } catch (e) {
      setError(sentence(errorMessage(e)));
      setSubmitting(false);
    }
  };

  return (
    <Modal
      title={`Edit ${server.name}`}
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void submit()} disabled={!valid} loading={submitting}>
            Save
          </Button>
        </>
      }
    >
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <Field label="Name" error={nameError}>
          {({ id, describedBy }) => <input id={id} data-autofocus aria-describedby={describedBy} value={name} onChange={(e) => setName(e.target.value)} className={inputCls} />}
        </Field>
        <Field label="Description (optional)">
          {({ id }) => <input id={id} value={description} onChange={(e) => setDescription(e.target.value)} className={inputCls} />}
        </Field>
        <Field label="Local port (empty or 0 = automatic)" error={portError} hint="The port on this PC that your game connects to.">
          {({ id, describedBy }) => (
            <input
              id={id}
              aria-describedby={describedBy}
              inputMode="numeric"
              disabled={live}
              value={localText}
              onChange={(e) => setLocalText(e.target.value)}
              placeholder="Automatic"
              className={`${inputCls} max-w-48`}
            />
          )}
        </Field>
        {live && (
          <p className="flex items-start gap-2.5 rounded-lg border border-warn/25 bg-warn/8 px-3.5 py-2.5 text-[13px] text-[#f6d38a]">
            <TriangleAlert className="mt-0.5 size-4 shrink-0 text-warn" aria-hidden />
            Disconnect from this server before changing the local port.
          </p>
        )}
        {error && (
          <p role="alert" className="rounded-lg border border-danger/30 bg-danger/8 px-3.5 py-2.5 text-[13px] text-danger">
            {error}
          </p>
        )}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}
