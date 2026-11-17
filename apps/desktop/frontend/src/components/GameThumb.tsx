import { Box, Cog, Cuboid, Gamepad2, type LucideIcon } from "lucide-react";
import { cn } from "../lib/format";

interface Look {
  icon: LucideIcon;
  gradient: string;
  label: string;
}

// Original abstract tiles, not game artwork.
const LOOKS: Record<string, Look> = {
  "minecraft-java": { icon: Box, gradient: "from-[#5b9a3a] to-[#7a5230]", label: "Minecraft Java" },
  "minecraft-bedrock": { icon: Cuboid, gradient: "from-[#14b8a6] to-[#2563eb]", label: "Minecraft Bedrock" },
  factorio: { icon: Cog, gradient: "from-[#f59e0b] to-[#c2410c]", label: "Factorio" },
  custom: { icon: Gamepad2, gradient: "from-[#5b6f94] to-[#33425f]", label: "Other game" },
};

export function GameThumb({ game, className }: { game: string; className?: string }) {
  const look = LOOKS[game] ?? LOOKS.custom;
  const Icon = look.icon;
  return (
    <div
      role="img"
      aria-label={look.label}
      className={cn("grid size-12 shrink-0 place-items-center rounded-xl bg-linear-to-br shadow-card ring-1 ring-white/10", look.gradient, className)}
    >
      <Icon className="size-6 text-white/90 drop-shadow-sm" strokeWidth={1.8} aria-hidden />
    </div>
  );
}
