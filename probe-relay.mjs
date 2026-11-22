for (let t = 0; t < 3; t++) {
  try {
    const r = await fetch("https://mcdles.us.ci/neoforge-relay/releases/net/neoforged/neoforge/maven-metadata.xml", { signal: AbortSignal.timeout(20000) });
    const text = await r.text();
    console.log("try", t, "status", r.status, text.length + "B", "has 21.1.267:", text.includes("21.1.267"));
    break;
  } catch (e) { console.log("try", t, "fail:", e.cause?.code || e.message.slice(0, 60)); await new Promise(r => setTimeout(r, 3000)); }
}
