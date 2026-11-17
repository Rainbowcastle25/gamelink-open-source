import { Header } from "../components/Header";
import { SettingsPanel } from "../components/SettingsPanel";

export function SettingsScreen() {
  return (
    <>
      <Header title="Settings" subtitle="Changes apply immediately" />
      <SettingsPanel />
    </>
  );
}
