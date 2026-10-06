import { parseAllDocuments } from "yaml";

/** Parse errors, one line each, so nothing malformed is sent. */
export function yamlErrors(text: string): string[] {
  return Array.from(parseAllDocuments(text)).flatMap((d) =>
    d.errors.map((e) => `line ${e.linePos?.[0].line ?? "?"}: ${e.message.split("\n")[0]}`),
  );
}
