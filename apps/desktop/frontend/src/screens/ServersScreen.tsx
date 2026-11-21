import { ChevronDown, FileInput, KeyRound, Plus } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { AddServerModal } from "../components/AddServerModal";
import { Button } from "../components/Button";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { DiagnosticsModal } from "../components/DiagnosticsModal";
import { EditServerModal } from "../components/EditServerModal";
import { Header } from "../components/Header";
import { ImportInviteModal } from "../components/ImportInviteModal";
import { ServerList, useServerList } from "../components/ServerList";
import { useNav } from "../lib/nav";
import { useAppState, useStore } from "../lib/store";

type ModalState = { kind: "import" } | { kind: "add" } | { kind: "edit" | "diagnostics" | "remove"; id: string } | null;

function AddMenu({ onImport, onAdd }: { onImport: () => void; onAdd: () => void }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  const item = "flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-sm transition-colors duration-150 hover:bg-white/8 focus-visible:bg-white/8";
  return (
    <div
      className="relative"
      ref={ref}
      onKeyDown={(e) => {
        if (e.key === "Escape" && open) {
          e.stopPropagation();
          setOpen(false);
        }
      }}
    >
      <Button variant="primary" aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((o) => !o)} icon={<Plus className="size-4" aria-hidden />}>
        Add server
        <ChevronDown className="size-4 opacity-70" aria-hidden />
      </Button>
      {open && (
        <div role="menu" aria-label="Add server" className="absolute right-0 z-30 mt-2 w-60 rounded-xl border border-line-strong bg-surface-2 p-1.5 shadow-pop animate-pop-in">
          <button
            type="button"
            role="menuitem"
            className={item}
            onClick={() => {
              setOpen(false);
              onImport();
            }}
          >
            <FileInput className="size-4 text-accent" aria-hidden /> Import invitation
          </button>
          <button
            type="button"
            role="menuitem"
            className={item}
            onClick={() => {
              setOpen(false);
              onAdd();
            }}
          >
            <KeyRound className="size-4 text-accent" aria-hidden /> Add manually
          </button>
        </div>
      )}
    </div>
  );
}

export function ServersScreen() {
  const { servers } = useAppState();
  const store = useStore();
  const list = useServerList();
  const { intent, consumeIntent } = useNav();
  const [modal, setModal] = useState<ModalState>(null);

  // Other screens can ask for a dialog to be open on arrival.
  useEffect(() => {
    if (intent === "import") setModal({ kind: "import" });
    else if (intent === "add") setModal({ kind: "add" });
    if (intent) consumeIntent();
  }, [intent, consumeIntent]);

  const close = () => setModal(null);
  const target = modal && "id" in modal ? servers.find((s) => s.id === modal.id) : undefined;

  return (
    <>
      <Header title="Servers" subtitle="Games you host and servers you can join.">
        <AddMenu onImport={() => setModal({ kind: "import" })} onAdd={() => setModal({ kind: "add" })} />
      </Header>

      <ServerList
        list={list}
        manage={{
          onEdit: (s) => setModal({ kind: "edit", id: s.id }),
          onRemove: (s) => setModal({ kind: "remove", id: s.id }),
          onDiagnostics: (s) => setModal({ kind: "diagnostics", id: s.id }),
        }}
      />

      {modal?.kind === "import" && <ImportInviteModal onClose={close} />}
      {modal?.kind === "add" && <AddServerModal onClose={close} />}
      {modal?.kind === "edit" && target && <EditServerModal server={target} onClose={close} />}
      {modal?.kind === "diagnostics" && target && <DiagnosticsModal server={target} onClose={close} />}
      {modal?.kind === "remove" && target && (
        <ConfirmDialog
          title="Remove server"
          message={`Remove ${target.name}? You will need the invitation again to re-add it.`}
          confirmLabel="Remove"
          onClose={close}
          onConfirm={async () => {
            if (await store.remove(target.id, target.name)) close();
          }}
        />
      )}
    </>
  );
}
