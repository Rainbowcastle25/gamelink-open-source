import type { GameId, Proto } from "./types";

export interface GameOption {
  id: GameId;
  label: string;
  /** Fixed protocol, or null when the user chooses (custom games). */
  proto: Proto | null;
  defaultPort: number;
}

/** What the "Add server" form offers. Mirrors internal/games on the Go side. */
export const GAME_OPTIONS: GameOption[] = [
  { id: "minecraft-java", label: "Minecraft: Java Edition", proto: "tcp", defaultPort: 25565 },
  { id: "minecraft-bedrock", label: "Minecraft: Bedrock Edition", proto: "udp", defaultPort: 19132 },
  { id: "factorio", label: "Factorio", proto: "udp", defaultPort: 34197 },
  { id: "custom", label: "Other", proto: null, defaultPort: 0 },
];

/**
 * One row per game type in Settings -> Local game executables. `noProgram`
 * says what Play does when no program is set; `argsHint` helps with the
 * connect arguments.
 */
export const EXECUTABLE_GAMES: Array<{ id: GameId; label: string; noProgram?: string; argsHint: string }> = [
  {
    id: "minecraft-java",
    label: "Minecraft Java",
    argsHint:
      "With the multiplayer list enabled, choose your GameLink entry in Minecraft. Otherwise Play copies the address. Prism can join directly with -l <instance> -s {address}.",
  },
  {
    id: "minecraft-bedrock",
    label: "Minecraft Bedrock",
    noProgram: "Not needed: Play opens Minecraft with a minecraft:// link that joins the server.",
    argsHint: "Only used if you pick a program instead of the built-in link.",
  },
  { id: "factorio", label: "Factorio", argsHint: "--mp-connect {address} makes Factorio join the server on start. Pick factorio.exe, not a Steam shortcut." },
  { id: "custom", label: "Other", argsHint: "Use {address} (127.0.0.1:port), {host} or {port} wherever your game takes a server address." },
];

export function protoLabel(p: string): string {
  return p.toUpperCase();
}
