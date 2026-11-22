const urls = [
  "https://neoforged.net/",
  "https://neoforged.net/installers/",
  "https://neoforged.net/api/versions",
  "https://neoforged.net/feed/maven/net/neoforged/neoforge",
];
for (const u of urls) {
  for (let t = 0; t < 2; t++) {
    try {
      const r = await fetch(u, { redirect: "follow", signal: AbortSignal.timeout(25000) });
      const text = await r.text();
      console.log(u, "->", r.status, text.length + "B");
      // 找版本/安装器线索
      const hits = text.match(/neoforge-\d+\.\d+\.\d+[^"'<]*installer[^"'<]*\.jar/g);
      const api = text.match(/(\/api\/[a-z0-9/_.-]*version[a-z0-9/_.-]*)/gi);
      if (hits) console.log("  installer links:", hits.slice(0, 3));
      if (api) console.log("  api paths:", [...new Set(api)].slice(0, 5));
      break;
    } catch (e) { console.log(u, "retry:", e.message.slice(0, 40)); await new Promise(r => setTimeout(r, 2000)); }
  }
}
