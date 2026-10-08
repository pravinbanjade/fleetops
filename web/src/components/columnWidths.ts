import { useEffect, useState } from "react";

/** The narrowest a column can be dragged: room for a holder's edge or a short number. */
export const MIN_WIDTH = 48;
/** The width a column shown after the widths were frozen starts at. */
export const NEW_COLUMN_WIDTH = 140;

export type Widths = Record<string, number>;

const key = (table: string) => `fleetops.columns.${table}`;

function load(table: string): Widths | null {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(key(table)) ?? "null");
    if (!v || typeof v !== "object") return null;
    const out: Widths = {};
    for (const [id, w] of Object.entries(v)) if (typeof w === "number" && w >= MIN_WIDTH) out[id] = Math.round(w);
    return Object.keys(out).length ? out : null;
  } catch {
    return null;
  }
}

/**
 * The column widths the operator dragged a table to, remembered per table in
 * this browser. `null` until the first resize: the table keeps its natural
 * layout until then.
 */
export function useColumnWidths(table: string) {
  const [widths, setWidths] = useState<Widths | null>(() => load(table));
  useEffect(() => {
    try {
      if (widths) localStorage.setItem(key(table), JSON.stringify(widths));
      else localStorage.removeItem(key(table));
    } catch {
      /* storage blocked: the widths last this page only */
    }
  }, [table, widths]);
  return [widths, setWidths] as const;
}
