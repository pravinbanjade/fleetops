import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

export type Theme = "light" | "dark" | "system";

const KEY = "fleetops-theme";

export function readTheme(): Theme {
  try {
    const t = localStorage.getItem(KEY);
    // Dark first: an operator who never chose sees the dark bay.
    return t === "light" || t === "dark" || t === "system" ? t : "dark";
  } catch {
    return "dark";
  }
}

export function resolveTheme(theme: Theme, prefersDark: boolean): "light" | "dark" {
  return theme === "system" ? (prefersDark ? "dark" : "light") : theme;
}

const query = () => window.matchMedia?.("(prefers-color-scheme: dark)");

const ThemeContext = createContext<{ theme: Theme; setTheme: (t: Theme) => void } | null>(null);

/** Applies the chosen theme as the `dark` class on <html> and remembers it. */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(readTheme);

  useEffect(() => {
    const mq = query();
    const apply = () => {
      const dark = resolveTheme(theme, mq?.matches ?? false) === "dark";
      document.documentElement.classList.toggle("dark", dark);
      document.documentElement.style.colorScheme = dark ? "dark" : "light";
    };
    apply();
    if (theme !== "system" || !mq) return;
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [theme]);

  const setTheme = (t: Theme) => {
    try {
      localStorage.setItem(KEY, t);
    } catch {
      // Storage may be unavailable (private mode); the choice lasts for this tab.
    }
    setThemeState(t);
  };

  return <ThemeContext value={{ theme, setTheme }}>{children}</ThemeContext>;
}

export function useTheme() {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme outside ThemeProvider");
  return ctx;
}
