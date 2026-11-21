const pairs = [["1.21.4","0.16.14"],["1.21.4","0.17.3"],["1.21.4","0.20.1"],["26.3","0.20.1"]];
for (const [mc, lv] of pairs) {
  for (let t = 0; t < 3; t++) {
    try {
      const r = await fetch(`https://meta.fabricmc.net/v2/versions/loader/${mc}/${lv}/profile`);
      console.log(mc, lv, "->", r.status);
      if (r.status === 200) {
        const j = await r.json();
        console.log("  id:", j.id, "mainClass:", j.mainClass);
      }
      break;
    } catch (e) { await new Promise(r=>setTimeout(r,2000)); console.log("retry", e.message.slice(0,40)); }
  }
}
