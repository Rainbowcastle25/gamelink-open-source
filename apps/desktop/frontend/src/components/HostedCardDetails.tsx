import { useState } from "react";
import { Button } from "./Button";
import { errorMessage } from "../lib/format";
import { toast } from "../lib/toast";

export function HostedCardDetails({ name, detail, copy }: {
  name: string;
  detail: string;
  copy?: () => Promise<void>;
}) {
  const [copying, setCopying] = useState(false);

  async function copyInvitation() {
    if (!copy || copying) return;
    setCopying(true);
    try {
      await copy();
      toast.success(`Invitation copied for ${name}. Share it privately.`);
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setCopying(false);
    }
  }

  return (
    <div className="flex flex-wrap items-center justify-between gap-2">
      <p className="text-[13px] text-muted">{detail}</p>
      {copy && <Button size="sm" loading={copying} disabled={copying} onClick={() => void copyInvitation()}>Copy invitation</Button>}
    </div>
  );
}
