const clamp = (value: number, min: number, max: number) =>
  Math.min(max, Math.max(min, value));

function parseHex(hex: string) {
  const normalized = hex.replace("#", "");
  if (!/^[0-9a-f]{6}$/i.test(normalized)) {
    throw new Error(`Invalid HEX color: ${hex}`);
  }

  return {
    red: Number.parseInt(normalized.slice(0, 2), 16) / 255,
    green: Number.parseInt(normalized.slice(2, 4), 16) / 255,
    blue: Number.parseInt(normalized.slice(4, 6), 16) / 255,
  };
}

function srgbChannelToLinear(channel: number) {
  return channel <= 0.04045
    ? channel / 12.92
    : ((channel + 0.055) / 1.055) ** 2.4;
}

function hexToLab(hex: string) {
  const rgb = parseHex(hex);
  const red = srgbChannelToLinear(rgb.red);
  const green = srgbChannelToLinear(rgb.green);
  const blue = srgbChannelToLinear(rgb.blue);

  const x = (red * 0.4124 + green * 0.3576 + blue * 0.1805) / 0.95047;
  const y = red * 0.2126 + green * 0.7152 + blue * 0.0722;
  const z = (red * 0.0193 + green * 0.1192 + blue * 0.9505) / 1.08883;

  const pivot = (value: number) =>
    value > 0.008856 ? Math.cbrt(value) : 7.787 * value + 16 / 116;

  const fx = pivot(x);
  const fy = pivot(y);
  const fz = pivot(z);

  return {
    lightness: 116 * fy - 16,
    a: 500 * (fx - fy),
    b: 200 * (fy - fz),
  };
}

const degreesToRadians = (degrees: number) => (degrees * Math.PI) / 180;
const radiansToDegrees = (radians: number) => (radians * 180) / Math.PI;

export function deltaE00(firstHex: string, secondHex: string) {
  const first = hexToLab(firstHex);
  const second = hexToLab(secondHex);
  const c1 = Math.hypot(first.a, first.b);
  const c2 = Math.hypot(second.a, second.b);
  const meanC = (c1 + c2) / 2;
  const compensation =
    0.5 * (1 - Math.sqrt(meanC ** 7 / (meanC ** 7 + 25 ** 7)));
  const a1Prime = (1 + compensation) * first.a;
  const a2Prime = (1 + compensation) * second.a;
  const c1Prime = Math.hypot(a1Prime, first.b);
  const c2Prime = Math.hypot(a2Prime, second.b);

  const hue = (a: number, b: number) => {
    const angle = radiansToDegrees(Math.atan2(b, a));
    return angle >= 0 ? angle : angle + 360;
  };

  const h1Prime = c1Prime === 0 ? 0 : hue(a1Prime, first.b);
  const h2Prime = c2Prime === 0 ? 0 : hue(a2Prime, second.b);
  const deltaLightness = second.lightness - first.lightness;
  const deltaChroma = c2Prime - c1Prime;
  const rawHueDifference = h2Prime - h1Prime;
  const deltaHueDegrees =
    c1Prime * c2Prime === 0
      ? 0
      : Math.abs(rawHueDifference) <= 180
        ? rawHueDifference
        : rawHueDifference > 180
          ? rawHueDifference - 360
          : rawHueDifference + 360;
  const deltaHue =
    2 * Math.sqrt(c1Prime * c2Prime) * Math.sin(degreesToRadians(deltaHueDegrees / 2));
  const meanLightness = (first.lightness + second.lightness) / 2;
  const meanChroma = (c1Prime + c2Prime) / 2;
  const meanHue =
    c1Prime * c2Prime === 0
      ? h1Prime + h2Prime
      : Math.abs(h1Prime - h2Prime) <= 180
        ? (h1Prime + h2Prime) / 2
        : h1Prime + h2Prime < 360
          ? (h1Prime + h2Prime + 360) / 2
          : (h1Prime + h2Prime - 360) / 2;
  const hueWeight =
    1 -
    0.17 * Math.cos(degreesToRadians(meanHue - 30)) +
    0.24 * Math.cos(degreesToRadians(2 * meanHue)) +
    0.32 * Math.cos(degreesToRadians(3 * meanHue + 6)) -
    0.2 * Math.cos(degreesToRadians(4 * meanHue - 63));
  const lightnessWeight =
    1 +
    (0.015 * (meanLightness - 50) ** 2) /
      Math.sqrt(20 + (meanLightness - 50) ** 2);
  const chromaWeight = 1 + 0.045 * meanChroma;
  const hueScale = 1 + 0.015 * meanChroma * hueWeight;
  const rotationAngle =
    30 * Math.exp(-(((meanHue - 275) / 25) ** 2));
  const chromaRotation =
    2 * Math.sqrt(meanChroma ** 7 / (meanChroma ** 7 + 25 ** 7));
  const rotationTerm =
    -Math.sin(degreesToRadians(2 * rotationAngle)) * chromaRotation;
  const lightnessTerm = deltaLightness / lightnessWeight;
  const chromaTerm = deltaChroma / chromaWeight;
  const hueTerm = deltaHue / hueScale;

  return clamp(
    Math.sqrt(
      lightnessTerm ** 2 +
        chromaTerm ** 2 +
        hueTerm ** 2 +
        rotationTerm * chromaTerm * hueTerm,
    ),
    0,
    100,
  );
}

export function qualityForDelta(delta: number) {
  if (delta <= Number.EPSILON) return "Exact" as const;
  if (delta <= 3) return "Close" as const;
  if (delta <= 6) return "Review" as const;
  return "Poor" as const;
}
