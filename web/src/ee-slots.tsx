import { useQuery } from "@tanstack/react-query";
import { createElement } from "react";
import { ee } from "@ee";
import type { Feature } from "@/api/gen/types.gen";
import { editionQuery } from "@/api/queries";
import type { EeNavItem, EeSlot, SlotName, SlotProps } from "./ee-contract";

// The only module that imports `@ee` (scripts/check-editions.sh).

export { ee };
export const isEnterpriseBuild = ee.id !== "ce";

/** The Enterprise features in effect; empty, with no request, in a Community build. */
export function useFeatures(): readonly Feature[] {
  const { data } = useQuery({ ...editionQuery, enabled: isEnterpriseBuild });
  return data?.features ?? [];
}

const allowed = (features: readonly Feature[], need?: Feature) => !need || features.includes(need);

/** The Enterprise component for `name`, when this build has one and the license covers it. */
export function Slot<N extends SlotName>({ name, props }: { name: N; props?: SlotProps[N] }) {
  const features = useFeatures();
  const slot = ee.slots[name] as EeSlot<N> | undefined;
  if (!slot || !allowed(features, slot.feature)) return null;
  return createElement(slot.component, props ?? ({} as SlotProps[N]));
}

export function useEeAdminNav(): EeNavItem[] {
  const features = useFeatures();
  return ee.adminNav.filter((n) => allowed(features, n.feature));
}
