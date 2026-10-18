// The licenses of the JavaScript packages that end up in the window (npm production dependencies), as text.
//   node scripts/third-party.mjs > js-licenses.txt
import { execSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const paths = execSync("npm ls --omit=dev --all --parseable", { encoding: "utf8", maxBuffer: 1 << 26 })
  .split("\n")
  .filter((p) => p && p.includes("node_modules"));
const seen = new Set();
const out = [];
for (const path of paths.sort()) {
  const meta = JSON.parse(readFileSync(join(path, "package.json"), "utf8"));
  const key = `${meta.name}@${meta.version}`;
  if (seen.has(key)) continue;
  seen.add(key);
  const file = readdirSync(path).find((f) => /^(licen[sc]e|copying)/i.test(f));
  const license = typeof meta.license === "string" ? meta.license : JSON.stringify(meta.license ?? meta.licenses);
  const text = file ? readFileSync(join(path, file), "utf8").trim() : `(no license file in the package; it declares ${license})`;
  out.push(`================================================================================\n${key} (${license})${meta.repository?.url ? `\n${meta.repository.url}` : ""}\n\n${text}\n`);
}
process.stdout.write(`JavaScript packages\n\n${out.join("\n")}`);
