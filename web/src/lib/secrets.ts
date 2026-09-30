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
