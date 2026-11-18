import { Box, Cog, Cuboid, Gamepad2, type LucideIcon } from "lucide-react";
import { useState } from "react";
import { cn } from "../lib/format";

interface Look {
  icon: LucideIcon;
  gradient: string;
  label: string;
}

const LOOKS: Record<string, Look> = {
  "minecraft-java": { icon: Box, gradient: "from-[#5b9a3a] to-[#7a5230]", label: "Minecraft Java" },
  "minecraft-bedrock": { icon: Cuboid, gradient: "from-[#14b8a6] to-[#2563eb]", label: "Minecraft Bedrock" },
  factorio: { icon: Cog, gradient: "from-[#f59e0b] to-[#c2410c]", label: "Factorio" },
  custom: { icon: Gamepad2, gradient: "from-[#5b6f94] to-[#33425f]", label: "Other game" },
};

export function GameThumb({ game, src, className }: { game: string; src?: string; className?: string }) {
  const look = LOOKS[game] ?? LOOKS.custom;
  const Icon = look.icon;
  const [failed, setFailed] = useState<string>();
  const hasIcon = src && src !== failed;
  return (
    <div
      role="img"
      aria-label={look.label}
      className={cn("grid size-12 shrink-0 place-items-center rounded-xl shadow-card ring-1 ring-white/10", hasIcon ? "bg-white/5" : cn("bg-linear-to-br", look.gradient), className)}
    >
      {hasIcon ? (
        <img src={src} alt="" className="size-4/5 object-contain" onError={() => setFailed(src)} />
      ) : (
        <Icon className="size-6 text-white/90 drop-shadow-sm" strokeWidth={1.8} aria-hidden />
      )}
    </div>
  );
}
