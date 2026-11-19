import { ErrorBoundary } from "./components/ErrorBoundary";
import { MockRibbon } from "./components/MockRibbon";
import { ReleaseNotifications } from "./components/ReleaseNotifications";
import { Sidebar } from "./components/Sidebar";
import { Toaster } from "./components/Toaster";
import { isMock, type Backend } from "./lib/backend";
import { NavProvider, useNav } from "./lib/nav";
import { GameLinkProvider } from "./lib/store";
import { DashboardScreen } from "./screens/DashboardScreen";
import { PeopleScreen } from "./screens/PeopleScreen";
import { HostScreen } from "./screens/HostScreen";
import { LocalHostScreen } from "./screens/LocalHostScreen";
import { ServersScreen } from "./screens/ServersScreen";
import { SettingsScreen } from "./screens/SettingsScreen";

function Screen() {
  const { screen } = useNav();
  switch (screen) {
    case "servers":
      return <ServersScreen />;
    case "host":
      return <LocalHostScreen />;
    case "people":
      return <PeopleScreen />;
    case "host-admin":
      return <HostScreen />;
    case "settings":
      return <SettingsScreen />;
    default:
      return <DashboardScreen />;
  }
}

interface Props {
  /** Tests inject a backend; the app normally uses the one chosen in lib/backend.ts. */
  backend?: Backend;
  mock?: boolean;
}

export default function App({ backend, mock = isMock }: Props) {
  return (
    <ErrorBoundary>
      <GameLinkProvider backend={backend}>
        <NavProvider>
          <div className="flex h-full">
            <Sidebar />
            <div className="flex min-w-0 flex-1 flex-col">
              <ReleaseNotifications />
              <main className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto">
                <div className="mx-auto w-full max-w-[1180px] px-8 py-8">
                  <Screen />
                </div>
              </main>
            </div>
          </div>
          <Toaster />
          {mock && <MockRibbon />}
        </NavProvider>
      </GameLinkProvider>
    </ErrorBoundary>
  );
}
