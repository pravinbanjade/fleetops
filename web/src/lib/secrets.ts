const SECRET = /PASS|SECRET|TOKEN|KEY/i;

/** Env keys whose values the UI masks until clicked: `*PASS*`, `*SECRET*`, `*TOKEN*`, `*KEY*`. */
export function isSecretKey(key: string) {
  return SECRET.test(key);
}

/** `KEY=value` into its parts; the value may itself contain `=`. */
export function splitEnv(entry: string): [string, string] {
  const i = entry.indexOf("=");
  return i < 0 ? [entry, ""] : [entry.slice(0, i), entry.slice(i + 1)];
}

/** What a masked value shows. */
export const MASK = "••••••••";

/** A driver option (e.g. a CIFS `o`) whose value carries a password or key. */
export function isSecretOption(key: string, value: string) {
  return isSecretKey(key) || /(PASS|SECRET|TOKEN|KEY)[^=,]*=/i.test(value);
}

/** `KEY=value` entries with secret-looking values replaced by the mask. */
export function maskEnv(env: string[]): string[] {
  return env.map((e) => {
    const [k] = splitEnv(e);
    return isSecretKey(k) && e.includes("=") ? `${k}=${MASK}` : e;
  });
}

/** Options with secret-looking values replaced by the mask. */
export function maskOptions(options: Record<string, string> | null | undefined) {
  if (!options) return options;
  return Object.fromEntries(Object.entries(options).map(([k, v]) => [k, isSecretOption(k, v) ? MASK : v]));
}
