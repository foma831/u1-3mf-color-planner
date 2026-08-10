import type { CSSProperties } from "react";

interface ColorSwatchProps {
  hex: string;
  size?: "small" | "medium" | "large";
}

export function ColorSwatch({ hex, size = "medium" }: ColorSwatchProps) {
  return (
    <span
      aria-hidden="true"
      className={`color-swatch color-swatch--${size}`}
      style={{ "--swatch-color": hex } as CSSProperties}
    />
  );
}
