import { Inbox, LayoutDashboard, RadioTower, Server, Settings, Users, type LucideIcon } from "lucide-react";
import { cn } from "../lib/format";
import { useNav } from "../lib/nav";
import { useAppState } from "../lib/store";
import type { Screen } from "../lib/types";
import { Logo } from "./Logo";

const ITEMS: Array<{ id: Screen; label: string; icon: LucideIcon }> = [
  { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
  { id: "servers", label: "Servers", icon: Server },
  { id: "requests", label: "Requests", icon: Inbox },
  { id: "friends", label: "Friends", icon: Users },
  { id: "host", label: "Host admin", icon: RadioTower },
  { id: "settings", label: "Settings", icon: Settings },
];

export function Sidebar() {
  const { screen, go } = useNav();
  const { version } = useAppState();
  return (
    <aside className="flex w-[200px] shrink-0 flex-col border-r border-line bg-[#080e1c] px-3 pt-6 pb-4">
      <div className="px-2 pb-7">
        <Logo />
      </div>
      <nav aria-label="Main" className="flex flex-1 flex-col gap-1">
        {ITEMS.map(({ id, label, icon: Icon }) => {
          const active = screen === id;
          return (
            <button
              key={id}
              type="button"
              aria-current={active ? "page" : undefined}
              onClick={() => go(id)}
              className={cn(
                "flex items-center gap-3 rounded-lg px-3 py-2 text-left text-sm font-medium transition-colors duration-150",
                active ? "bg-teal/14 text-fg shadow-[inset_2px_0_0_var(--color-teal)]" : "text-muted hover:bg-white/5 hover:text-fg",
              )}
            >
              <Icon className={cn("size-[18px]", active ? "text-accent" : "text-faint")} aria-hidden />
              {label}
            </button>
          );
        })}
      </nav>
      <p className="px-2 text-xs text-faint">{version ? `${version.app} ${version.version}` : "GameLink"}</p>
    </aside>
  );
}
