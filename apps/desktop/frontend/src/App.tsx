import { ErrorBoundary } from "./components/ErrorBoundary";
import { MockRibbon } from "./components/MockRibbon";
import { Sidebar } from "./components/Sidebar";
import { Toaster } from "./components/Toaster";
import { isMock, type Backend } from "./lib/backend";
import { NavProvider, useNav } from "./lib/nav";
import { GameLinkProvider } from "./lib/store";
import { DashboardScreen } from "./screens/DashboardScreen";
import { FriendsScreen } from "./screens/FriendsScreen";
import { HostScreen } from "./screens/HostScreen";
import { RequestsScreen } from "./screens/RequestsScreen";
import { ServersScreen } from "./screens/ServersScreen";
import { SettingsScreen } from "./screens/SettingsScreen";

function Screen() {
  const { screen } = useNav();
  switch (screen) {
    case "servers":
      return <ServersScreen />;
    case "requests":
      return <RequestsScreen />;
    case "friends":
      return <FriendsScreen />;
    case "host":
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
            <main className="min-w-0 flex-1 overflow-x-hidden overflow-y-auto">
              <div className="mx-auto w-full max-w-[1180px] px-8 py-8">
                <Screen />
              </div>
            </main>
          </div>
          <Toaster />
          {mock && <MockRibbon />}
        </NavProvider>
      </GameLinkProvider>
    </ErrorBoundary>
  );
}
