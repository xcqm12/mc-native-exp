import fs from "node:fs";
const urls = fs.readFileSync("data/neoforge-lib-urls.txt", "utf8").trim().split("\n").filter(u => u.trim());
const ok = [], miss = [];
for (const u of urls) {
  const rel = u.replace("https://maven.neoforged.net/releases/", "");
  const central = `https://repo1.maven.org/maven2/${rel}`;
  let status = 0;
  for (let t = 0; t < 2; t++) {
    try {
      const r = await fetch(central, { method: "HEAD", signal: AbortSignal.timeout(15000) });
      status = r.status;
      break;
    } catch (e) { await new Promise(r2 => setTimeout(r2, 1500)); }
  }
  (status === 200 ? ok : miss).push(`${status} ${rel}`);
}
console.log("central OK:", ok.length, " miss:", miss.length);
for (const m of miss) console.log("  ", m);
fs.writeFileSync("data/neoforge-central-ok.txt", ok.map(l => l.replace(/^\d+ /, "")).join("\n"));
