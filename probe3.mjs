// fabric installer 最新版（maven-metadata）
for (const url of ["https://maven.fabricmc.net/net/fabricmc/fabric-installer/maven-metadata.xml"]) {
  for (let t = 0; t < 3; t++) {
    try {
      const r = await fetch(url);
      const text = await r.text();
      const vers = [...text.matchAll(/<version>([^<]+)<\/version>/g)].map(m => m[1]);
      console.log(url.split("fabricmc")[0] + "…", "status", r.status, "latest:", vers.at(-1), "release:", /<release>([^<]+)<\/release>/.exec(text)?.[1]);
      const rel = /<release>([^<]+)<\/release>/.exec(text)?.[1] || vers.at(-1);
      const jar = `https://maven.fabricmc.net/net/fabricmc/fabric-installer/${rel}/fabric-installer-${rel}.jar`;
      const jr = await fetch(jar, { method: "HEAD" });
      console.log("installer jar:", jr.status, jr.headers.get("content-length"));
      break;
    } catch (e) { console.log("retry", e.message.slice(0,40)); await new Promise(r=>setTimeout(r,2000)); }
  }
}
