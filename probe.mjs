const base = "https://meta.fabricmc.net/v2/versions/loader";
for (const mc of ["1.21.4", "26.3"]) {
  for (let t = 0; t < 3; t++) {
    try {
      const r = await fetch(`${base}/${mc}`);
      const j = await r.json();
      console.log(mc, "status", r.status, "entries", Array.isArray(j) ? j.length : "?", 
        "first:", j?.[0]?.loader?.version, "stable:", j?.[0]?.loader?.stable);
      if (Array.isArray(j) && j.length) {
        const v = j[0].loader.version;
        const p = await fetch(`${base}/${mc}/${v}/profile`);
        console.log("  profile", p.status);
      }
      break;
    } catch (e) { console.log(mc, "retry", t, e.message.slice(0,50)); await new Promise(r=>setTimeout(r,3000)); }
  }
}
