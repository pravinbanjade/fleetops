import { act, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { readTheme, resolveTheme, ThemeProvider, useTheme, type Theme } from "./theme";

function Probe({ onReady }: { onReady: (set: (t: Theme) => void) => void }) {
  onReady(useTheme().setTheme);
  return null;
}

describe("theme", () => {
  it("resolves system from the OS preference", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
    expect(resolveTheme("light", true)).toBe("light");
  });

  it("applies and remembers the chosen theme", () => {
    let set!: (t: Theme) => void;
    render(
      <ThemeProvider>
        <Probe onReady={(s) => (set = s)} />
      </ThemeProvider>,
    );
    act(() => set("dark"));
    expect(document.documentElement).toHaveClass("dark");
    expect(readTheme()).toBe("dark");
    act(() => set("system"));
    expect(document.documentElement).not.toHaveClass("dark");
    expect(readTheme()).toBe("system");
  });
});
