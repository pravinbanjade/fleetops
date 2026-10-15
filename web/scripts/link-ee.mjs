// The Enterprise UI module has no package.json of its own; it resolves this
// package's dependencies through a git-ignored node_modules link.
import fs from "node:fs";
import path from "node:path";

const web = path.resolve(import.meta.dirname, "..");
const link = path.resolve(web, "../ee/web/node_modules");
if (!fs.existsSync(link)) {
  fs.mkdirSync(path.dirname(link), { recursive: true });
  fs.symlinkSync(path.join(web, "node_modules"), link, "dir");
}
