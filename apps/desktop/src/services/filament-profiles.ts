import type { PhysicalSpool } from "../types";

export const RELI3D_PVA_PROFILE = "Reli3D PVA @U1";
export const SNAPMAKER_PVA_PROFILE = "Snapmaker PVA @U1";

export const QUALIFIED_PVA_PROFILES = [
  RELI3D_PVA_PROFILE,
  SNAPMAKER_PVA_PROFILE,
] as const;

export function hasQualifiedPvaProfile(
  spool: Pick<PhysicalSpool, "material" | "profile">,
) {
  return (
    spool.material === "PVA" &&
    QUALIFIED_PVA_PROFILES.includes(
      spool.profile as (typeof QUALIFIED_PVA_PROFILES)[number],
    )
  );
}
