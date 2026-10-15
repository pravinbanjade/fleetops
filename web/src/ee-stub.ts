import type { EeModule } from "./ee-contract";

/** The Community build's stand-in for the Enterprise UI module: nothing. */
export const ee: EeModule = { id: "ce", adminRoutes: () => [], adminNav: [], slots: {} };
