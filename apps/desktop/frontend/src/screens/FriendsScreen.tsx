import { Bell, UserPlus, Users, Wifi } from "lucide-react";
import { Header } from "../components/Header";

const PLANNED = [
  { icon: Users, title: "Friend list", text: "Keep the people you play with in one place." },
  { icon: Bell, title: "Request approvals from inside the app", text: "Approve a friend's device without using the command line." },
  { icon: Wifi, title: "Shared server presence", text: "See which of your shared servers are up before you join." },
];

export function FriendsScreen() {
  return (
    <>
      <Header title="Friends" subtitle="Coming later" />
      <section aria-labelledby="friends-later" className="rounded-xl border border-line bg-surface p-8 shadow-card">
        <div className="mb-4 grid size-12 place-items-center rounded-xl bg-white/5 text-accent">
          <UserPlus className="size-6" aria-hidden />
        </div>
        <h2 id="friends-later" className="text-lg font-semibold">
          Friends are coming later
        </h2>
        <p className="mt-2 max-w-xl text-sm leading-relaxed text-muted">
          This alpha has no accounts, presence or friend lists. You connect to a friend's server with an invitation they send you, and nothing about who you play with is stored anywhere but your own PC.
        </p>
        <ul className="mt-6 divide-y divide-line rounded-lg border border-line">
          {PLANNED.map(({ icon: Icon, title, text }) => (
            <li key={title} className="flex items-center gap-4 px-4 py-3.5">
              <Icon className="size-5 shrink-0 text-faint" aria-hidden />
              <div className="min-w-0 flex-1">
                <p className="text-sm font-medium">{title}</p>
                <p className="text-[13px] text-muted">{text}</p>
              </div>
              <span className="shrink-0 rounded-md border border-line-strong bg-white/4 px-2 py-1 text-xs text-muted">Planned for a future release</span>
            </li>
          ))}
        </ul>
      </section>
    </>
  );
}
