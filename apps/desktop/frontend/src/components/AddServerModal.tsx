import { Eye, EyeOff } from "lucide-react";
import { useState } from "react";
import { errorMessage, parseOptionalPort, parseRequiredPort, sentence } from "../lib/format";
import { GAME_OPTIONS } from "../lib/games";
import { useStore } from "../lib/store";
import { toast } from "../lib/toast";
import type { GameId, Proto } from "../lib/types";
import { Button } from "./Button";
import { Field, inputCls } from "./Field";
import { Modal } from "./Modal";

export function AddServerModal({ onClose }: { onClose: () => void }) {
  const store = useStore();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [game, setGame] = useState<GameId>("minecraft-java");
  const [proto, setProto] = useState<Proto>("tcp");
  const [portText, setPortText] = useState("25565");
  const [localText, setLocalText] = useState("");
  const [address, setAddress] = useState("");
  const [reveal, setReveal] = useState(false);
  const [touched, setTouched] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const option = GAME_OPTIONS.find((g) => g.id === game) ?? GAME_OPTIONS[0];
  const effectiveProto: Proto = option.proto ?? proto;

  const onGame = (id: GameId) => {
    const o = GAME_OPTIONS.find((g) => g.id === id) ?? GAME_OPTIONS[0];
    setGame(id);
    if (o.proto) setProto(o.proto);
    if (o.defaultPort) setPortText(String(o.defaultPort));
    else setPortText("");
  };

  const port = parseRequiredPort(portText);
  const localPort = parseOptionalPort(localText);
  const errors = {
    name: name.trim() ? null : "Give the server a name.",
    port: port === null ? "Enter a port between 1 and 65535." : null,
    local: localPort === null ? "Enter a port between 1 and 65535, or leave this empty for automatic." : null,
    address: address.trim() ? null : "Paste the server address you received from the owner.",
  };
  const valid = !errors.name && !errors.port && !errors.local && !errors.address;
  const show = (k: keyof typeof errors) => (touched ? errors[k] : null);

  const submit = async () => {
    setTouched(true);
    if (!valid || port === null || localPort === null) return;
    setSubmitting(true);
    setError(null);
    try {
      const s = await store.addServer({
        name: name.trim(),
        description: description.trim(),
        game,
        proto: effectiveProto,
        port,
        localPort,
        address: address.trim(),
      });
      toast.success(`Added “${s.name}”`);
      onClose();
    } catch (e) {
      setError(sentence(errorMessage(e)));
      setSubmitting(false);
    }
  };

  return (
    <Modal
      title="Add server manually"
      onClose={onClose}
      width="max-w-xl"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void submit()} loading={submitting}>
            Add server
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
        noValidate
      >
        <Field label="Name" error={show("name")}>
          {({ id, describedBy }) => <input id={id} data-autofocus aria-describedby={describedBy} value={name} onChange={(e) => setName(e.target.value)} placeholder="Weekend server" className={inputCls} />}
        </Field>
        <Field label="Description (optional)">
          {({ id }) => <input id={id} value={description} onChange={(e) => setDescription(e.target.value)} placeholder="A short note for yourself" className={inputCls} />}
        </Field>

        <div className="grid grid-cols-2 gap-4">
          <Field label="Game">
            {({ id }) => (
              <select id={id} value={game} onChange={(e) => onGame(e.target.value as GameId)} className={inputCls}>
                {GAME_OPTIONS.map((g) => (
                  <option key={g.id} value={g.id}>
                    {g.label}
                  </option>
                ))}
              </select>
            )}
          </Field>
          <Field label="Protocol">
            {({ id }) => (
              <select id={id} value={effectiveProto} disabled={!!option.proto} onChange={(e) => setProto(e.target.value as Proto)} className={inputCls}>
                <option value="tcp">TCP</option>
                <option value="udp">UDP</option>
              </select>
            )}
          </Field>
        </div>

        <div className="grid grid-cols-2 gap-4">
          <Field label="Tunnel port" error={show("port")} hint="The port the game server listens on.">
            {({ id, describedBy }) => <input id={id} aria-describedby={describedBy} inputMode="numeric" value={portText} onChange={(e) => setPortText(e.target.value)} className={inputCls} />}
          </Field>
          <Field label="Local port (optional)" error={show("local")} hint="Leave empty for automatic.">
            {({ id, describedBy }) => (
              <input id={id} aria-describedby={describedBy} inputMode="numeric" value={localText} onChange={(e) => setLocalText(e.target.value)} placeholder="Automatic" className={inputCls} />
            )}
          </Field>
        </div>

        <Field
          label="Server address"
          error={show("address")}
          hint="This is a secret you got from the server owner. Anyone who has it and an approved device can reach the server, so do not share it."
        >
          {({ id, describedBy }) => (
            <div className="relative">
              <input
                id={id}
                aria-describedby={describedBy}
                type={reveal ? "text" : "password"}
                value={address}
                onChange={(e) => setAddress(e.target.value)}
                autoComplete="off"
                spellCheck={false}
                className={`${inputCls} pr-10 font-mono text-xs`}
              />
              <button
                type="button"
                aria-label={reveal ? "Hide server address" : "Show server address"}
                onClick={() => setReveal((r) => !r)}
                className="absolute top-1/2 right-1.5 grid size-7 -translate-y-1/2 place-items-center rounded-md text-muted hover:text-fg"
              >
                {reveal ? <EyeOff className="size-4" aria-hidden /> : <Eye className="size-4" aria-hidden />}
              </button>
            </div>
          )}
        </Field>

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
