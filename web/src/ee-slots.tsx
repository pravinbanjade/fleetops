import { useQuery } from "@tanstack/react-query";
import { createElement } from "react";
import { ee } from "@ee";
import type { Edition, Feature } from "@/api/gen/types.gen";
import { editionQuery } from "@/api/queries";
import { ADMIN_NAV } from "./components/nav";
import { ADMIN_GROUPS, type AdminGroup, type EeNavItem, type EeSlot, type RoleOption, type SlotName, type SlotProps } from "./ee-contract";

// The only module that imports `@ee` (scripts/check-editions.sh).

export { ee };

/** The plan in effect: Enterprise while a license is active; undefined until known. */
export function useEdition(): Edition | undefined {
  return useQuery(editionQuery).data?.edition;
}

/** The Enterprise features in effect; empty on Community. */
export function useFeatures(): readonly Feature[] {
  return useQuery(editionQuery).data?.features ?? [];
}

const PLAN_NAMES: Record<Edition, string> = { community: "Community", enterprise: "Enterprise" };

/** "Community 1.2.0" or "Enterprise 1.2.0"; the release alone until the plan is known. */
export function usePlanLabel(): string {
  const edition = useEdition();
  const version = import.meta.env.FLEETOPS_VERSION;
  return edition ? `${PLAN_NAMES[edition]} ${version}` : version;
}

const noLeftovers = (): Partial<Record<Feature, boolean>> => ({});
const leftovers = ee.useLeftovers ?? noLeftovers;

/** Whether a nav item or slot needing `need` shows: licensed, or leftover data to see and remove. */
function useShown(): (need?: Feature) => boolean {
  const features = useFeatures();
  const left = leftovers();
  return (need) => !need || features.includes(need) || left[need] === true;
}

/** The Enterprise component for `name`, when this build has one and the license covers it. */
export function Slot<N extends SlotName>({ name, props }: { name: N; props?: SlotProps[N] }) {
  const shown = useShown();
  const slot = ee.slots[name] as EeSlot<N> | undefined;
  if (!slot || !shown(slot.feature)) return null;
  return createElement(slot.component, props ?? ({} as SlotProps[N]));
}

/** Whether `name` would render: this build has it and the license (or leftover data) covers it. */
export function useSlotShown(name: SlotName): boolean {
  const shown = useShown();
  const slot = ee.slots[name] as EeSlot | undefined;
  return !!slot && shown(slot.feature);
}

const noRoles = (): RoleOption[] => [];
const roleOptions = ee.useRoleOptions ?? noRoles;

/** Custom roles the access editors offer after Viewer and Operator; none in Community. */
export function useRoleOptions(): RoleOption[] {
  return roleOptions();
}

export function useEeAdminNav(): EeNavItem[] {
  const shown = useShown();
  return ee.adminNav.filter((n) => shown(n.feature));
}

/** Every admin page, Community and Enterprise, grouped in nav order. */
export function useAdminNav(): { group: AdminGroup; items: EeNavItem[] }[] {
  const all: EeNavItem[] = [...ADMIN_NAV, ...useEeAdminNav()];
  return ADMIN_GROUPS.map((group) => ({ group, items: all.filter((n) => n.group === group) })).filter((g) => g.items.length);
}
