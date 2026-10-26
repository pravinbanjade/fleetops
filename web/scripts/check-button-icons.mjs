// Every text button carries an icon beside its label. Lists each <Button>
// whose children hold words but no capitalised JSX element (a Lucide icon),
// as file:line, and exits 1 when there is any. Icon-only buttons and
// buttons that wrap a component (asChild links) are checked the same way.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const roots = ["src", "../ee/web/src"];
const files = [];
const walk = (d) => {
  for (const n of readdirSync(d)) {
    const p = join(d, n);
    if (statSync(p).isDirectory()) walk(p);
    else if (p.endsWith(".tsx") && !p.endsWith(".test.tsx") && !p.includes("/components/ui/")) files.push(p);
  }
};
roots.forEach(walk);

const bad = [];
for (const f of files) {
  const src = readFileSync(f, "utf8");
  const re = /<Button\b([^>]*?)(\/?)>/gs;
  let m;
  while ((m = re.exec(src))) {
    if (m[2] === "/") continue;
    // The children run to the matching </Button>, counting nested Buttons.
    let depth = 1;
    let i = re.lastIndex;
    const tag = /<Button\b[^>]*?(\/?)>|<\/Button>/gs;
    tag.lastIndex = i;
    let t;
    while (depth && (t = tag.exec(src))) {
      if (t[0] === "</Button>") depth--;
      else if (t[1] !== "/") depth++;
    }
    if (!t) break;
    const body = src.slice(i, t.index);
    const words = body
      .replace(/<[^>]*>/g, " ")
      .replace(/\{[^{}]*\}/g, (x) => (/^\{\s*["'`]/.test(x) || /\?\s*["'`]/.test(x) ? " word " : " "))
      .trim();
    const hasWords = /[A-Za-z]{2,}/.test(words);
    const hasIcon = /<([A-Z][A-Za-z0-9]*|[a-z]+\.[A-Za-z]+)[\s/>]/.test(body);
    if (hasWords && !hasIcon) bad.push(`${relative(process.cwd(), f)}:${src.slice(0, m.index).split("\n").length}`);
  }
}
if (bad.length) {
  console.log(`${bad.length} button(s) with a label and no icon:`);
  console.log(bad.join("\n"));
  process.exit(1);
}
console.log("Every text button has an icon.");
