// La app se abre aunque no haya conexión (con lo último que vio); los datos siempre se piden al
// servidor. Cada versión nueva de la app reemplaza la anterior.
const CACHE = "notas-app-v1";
const SHELL = ["./", "app.js", "app.css", "manifest.webmanifest", "icon-192.png", "icon-512.png"];

self.addEventListener("install", (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches.keys().then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k)))).then(() => self.clients.claim())
  );
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  // Los datos (y todo lo que no es de la app) van directo al servidor.
  if (e.request.method !== "GET" || url.pathname.startsWith("/v1/") || !url.pathname.startsWith("/app")) return;
  // La app: primero la red (para tener siempre la última); sin red, la guardada.
  e.respondWith(
    fetch(e.request)
      .then((r) => {
        const copy = r.clone();
        caches.open(CACHE).then((c) => c.put(e.request, copy));
        return r;
      })
      .catch(() => caches.match(e.request).then((r) => r || caches.match("./")))
  );
});
