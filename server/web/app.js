// Notas en el celular: lo de hoy, tareas, agenda, hábitos, notas y la IA, sobre tu cuenta.
// Todo se lee y se escribe en el servidor (los mismos archivos de tus equipos); lo último que se
// vio queda guardado para abrir al instante.
"use strict";

const API = "/v1";
const EN = (navigator.language || "").toLowerCase().startsWith("en");

// ---------- Textos (español, y su inglés) ----------
const T_EN = {
  "Hoy": "Today", "Tareas": "Tasks", "Agenda": "Calendar", "Hábitos": "Habits", "Notas": "Notes", "IA": "AI",
  "Anota algo…": "Jot something down…", "Anotar": "Save", "Como tarea": "As a task",
  "Anotado en tu día: la IA lo organiza en tu computador": "Saved to your day: the AI will organize it on your computer",
  "Tarea agregada": "Task added", "Por agendar": "To schedule", "Agendar": "Schedule", "Descartar": "Dismiss",
  "Llegó por correo: no está en tu agenda hasta que la agendes.": "From an email: not on your calendar until you schedule it.",
  "Atrasadas": "Overdue", "Mañana": "Tomorrow", "Esta semana": "This week", "Sin fecha": "No date",
  "Nada para hoy.": "Nothing for today.", "Nota de hoy": "Today's note", "Hechas": "Done",
  "Nueva tarea (p. ej. Llamar a Pedro el viernes)": "New task (e.g. Call Pedro on Friday)", "Agregar": "Add",
  "No hay tareas pendientes.": "No pending tasks.", "Semana anterior": "Previous week", "Semana siguiente": "Next week",
  "Sin eventos ni tareas.": "No events or tasks.", "Hoy no toca ninguno de todos los días": "None of your daily habits today",
  "Meta de hoy cumplida": "Today's goal met", "Aún no tienes hábitos: créalos en Notas, en el computador.": "No habits yet: create them in Notes on your computer.",
  "Buscar en tus notas": "Search your notes", "Ninguna nota tiene eso.": "No note has that.", "Volver": "Back",
  "Agregar una línea…": "Add a line…", "Agregado a la nota": "Added to the note",
  "Pregúntale a la IA por tus notas, tareas o agenda": "Ask the AI about your notes, tasks or calendar",
  "¿Qué tengo hoy?": "What do I have today?", "¿Qué está atrasado?": "What's overdue?", "¿Qué tengo esta semana?": "What's on this week?",
  "Pensando…": "Thinking…", "Nueva conversación": "New conversation",
  "Entra con tu cuenta de Notas": "Sign in with your Notes account", "Tu correo": "Your email",
  "Mandarme un código": "Send me a code", "Código de 6 dígitos": "6-digit code", "Entrar": "Sign in",
  "Te mandamos un código a": "We sent a code to", "Usar mi clave": "Use my password", "Usar un código por correo": "Use an email code",
  "Tu clave": "Your password", "Salir": "Sign out", "Sin conexión: mostrando lo último que viste.": "Offline: showing what you last saw.",
  "Tu sesión terminó: vuelve a entrar.": "Your session ended: sign in again.", "Actualizar": "Refresh",
  "No se pudo": "Couldn't do it", "racha": "streak", "días": "days", "veces": "times", "semanas": "weeks",
  "esta semana": "this week", "de": "of", "Ver todo": "See all", "Pendientes": "Pending",
  "Para agregar hábitos o cambiarlos, usa Notas en el computador.": "To add or change habits, use Notes on your computer.",
  "Tus notas se sincronizan con tu cuenta: lo que hagas aquí aparece en tus equipos.": "Your notes sync with your account: what you do here shows up on your computers.",
  "Instalar: en el menú del navegador, «Agregar a pantalla de inicio».": "Install: in your browser menu, “Add to Home Screen”.",
};
const t = (s) => (EN && T_EN[s]) || s;

const DAYS_ES = ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"];
const DAYS_EN = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS_ES = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];
const MONTHS_EN = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

// ---------- Íconos ----------
const svg = (d, fill) => `<svg viewBox="0 0 24 24" fill="${fill ? "currentColor" : "none"}" stroke="${fill ? "none" : "currentColor"}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${d}</svg>`;
const I = {
  today: svg('<path d="M3 11l9-7 9 7"/><path d="M5 10v10h14V10"/>'),
  tasks: svg('<rect x="3" y="3" width="18" height="18" rx="4"/><path d="M8 12l3 3 5-6"/>'),
  agenda: svg('<rect x="3" y="5" width="18" height="16" rx="3"/><path d="M3 10h18M8 3v4M16 3v4"/>'),
  habits: svg('<path d="M12 3c1 4 5 5 5 10a5 5 0 0 1-10 0c0-2 1-3 2-4 0 2 1 3 2 3-1-3 0-6 1-9z"/>'),
  notes: svg('<path d="M6 3h9l4 4v14H6z"/><path d="M14 3v5h5M9 13h7M9 17h7"/>'),
  ai: svg('<path d="M12 3l2.2 5.8L20 11l-5.8 2.2L12 19l-2.2-5.8L4 11l5.8-2.2z"/>'),
  refresh: svg('<path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 4v7h-7"/>'),
  check: svg('<path d="M5 12l5 5 9-10"/>'),
  event: svg('<circle cx="12" cy="12" r="4"/>', true),
  mail: svg('<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 7l9 6 9-6"/>'),
  flame: svg('<path d="M12 3c1 4 5 5 5 10a5 5 0 0 1-10 0c0-2 1-3 2-4 0 2 1 3 2 3-1-3 0-6 1-9z"/>'),
  back: svg('<path d="M15 18l-6-6 6-6"/>'),
  send: svg('<path d="M4 12l16-8-6 16-2-6z"/>'),
  logout: svg('<path d="M15 4h4v16h-4M10 8l-4 4 4 4M6 12h10"/>'),
};

// ---------- Utilidades ----------
const $ = (s, el = document) => el.querySelector(s);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const pad = (n) => String(n).padStart(2, "0");
const ymd = (d) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const today = () => ymd(new Date());
const parseDay = (s) => { const [y, m, d] = s.split("-").map(Number); return new Date(y, m - 1, d); };
const addDays = (s, n) => { const d = parseDay(s); d.setDate(d.getDate() + n); return ymd(d); };
function longDate(s) {
  const d = parseDay(s);
  return EN ? `${DAYS_EN[d.getDay()]} ${MONTHS_EN[d.getMonth()]} ${d.getDate()}` : `${DAYS_ES[d.getDay()]} ${d.getDate()} ${MONTHS_ES[d.getMonth()]}`;
}
function whenText(s) {
  const now = today();
  if (s === now) return t("Hoy");
  if (s === addDays(now, 1)) return t("Mañana");
  return longDate(s);
}
const store = {
  get(k, d) { try { const v = localStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch {} },
  del(k) { try { localStorage.removeItem(k); } catch {} },
};
let toastTimer;
function toast(msg) {
  const el = $("#toast");
  el.textContent = msg;
  el.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove("show"), 2800);
}

// ---------- Servidor ----------
let token = store.get("token", "");
async function api(path, body, method) {
  const opts = { method: method || (body ? "POST" : "GET"), headers: { Authorization: `Bearer ${token}` } };
  if (body) { opts.headers["Content-Type"] = "application/json"; opts.body = JSON.stringify(body); }
  let r;
  try { r = await fetch(API + path, opts); } catch { throw new Error("offline"); }
  if (r.status === 401 && token) { signOut(true); throw new Error(t("Tu sesión terminó: vuelve a entrar.")); }
  const data = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(data?.error?.message || `${t("No se pudo")} (${r.status})`);
  return data;
}
function fail(e) {
  toast(e.message === "offline" ? t("Sin conexión: mostrando lo último que viste.") : e.message);
}

// ---------- Navegación ----------
const TABS = [
  ["hoy", "Hoy", I.today], ["tareas", "Tareas", I.tasks], ["agenda", "Agenda", I.agenda],
  ["habitos", "Hábitos", I.habits], ["notas", "Notas", I.notes], ["ia", "IA", I.ai],
];
let tab = store.get("tab", "hoy");
let openNote = null; // la nota abierta en «Notas»
const view = () => $("#view");

function drawTabs() {
  const nav = $("#tabs");
  nav.hidden = false;
  nav.innerHTML = TABS.map(([id, label, icon]) => `<button data-tab="${id}" ${tab === id ? 'aria-current="page"' : ""}>${icon}<span>${esc(t(label))}</span></button>`).join("");
  nav.querySelectorAll("button").forEach((b) => b.addEventListener("click", () => {
    if (tab === b.dataset.tab && b.dataset.tab === "notas") openNote = null;
    tab = b.dataset.tab;
    store.set("tab", tab);
    drawTabs();
    show();
    window.scrollTo(0, 0);
  }));
}

function setTitle(text) { $("#title").textContent = text; }

async function show() {
  const draw = { hoy: drawToday, tareas: drawTasks, agenda: drawAgenda, habitos: drawHabits, notas: drawNotes, ia: drawAsk }[tab] || drawToday;
  await draw();
}

// Cada pantalla dibuja primero lo guardado y después lo que llega.
async function load(key, path, draw) {
  const cached = store.get(key, null);
  if (cached) draw(cached);
  const btn = $("#refresh");
  btn.classList.add("spin");
  try {
    const data = await api(path);
    store.set(key, data);
    draw(data);
  } catch (e) {
    if (!cached) view().innerHTML = `<p class="empty">${esc(e.message === "offline" ? t("Sin conexión: mostrando lo último que viste.") : e.message)}</p>`;
    else if (e.message !== "offline") fail(e);
  } finally {
    btn.classList.remove("spin");
  }
}

// ---------- Piezas ----------
function taskItem(task, opts = {}) {
  const late = task.fecha && task.fecha < today() && !task.hecha;
  const meta = [];
  if (task.fecha && !opts.noDate) meta.push(`<span class="${late ? "late" : ""}">${esc(whenText(task.fecha))}</span>`);
  if (task.espacio) meta.push(esc(task.espacio));
  return `<div class="item ${task.hecha ? "done" : ""}">
    <button class="check ${task.hecha ? "on" : ""}" data-done="${esc(task.clave)}" data-state="${task.hecha ? 1 : 0}" aria-label="${task.hecha ? "✓" : "○"} ${esc(task.texto)}">${task.hecha ? I.check : ""}</button>
    <div class="body"><div class="text">${esc(task.texto)}</div>${meta.length ? `<div class="meta">${meta.join(" · ")}</div>` : ""}</div>
  </div>`;
}
function eventItem(e, withDay) {
  const when = withDay ? `${esc(whenText(e.fecha))}${e.hora ? " · " + esc(e.hora) : ""}` : "";
  return `<div class="item ${e.hecho ? "done" : ""}">
    ${!withDay && e.hora ? `<span class="time">${esc(e.hora)}</span>` : `<span class="dot">${I.event}</span>`}
    <div class="body"><div class="text">${esc(e.titulo)}</div><div class="meta">${[when, esc(e.espacio)].filter(Boolean).join(" · ")}</div></div>
  </div>`;
}
function wireTasks(root, after) {
  root.querySelectorAll("[data-done]").forEach((b) => b.addEventListener("click", async () => {
    const done = b.dataset.state !== "1";
    b.classList.toggle("on", done);
    b.innerHTML = done ? I.check : "";
    b.closest(".item").classList.toggle("done", done);
    try {
      await api("/movil/tarea/hecha", { hoy: today(), clave: b.dataset.done, hecha: done });
      after && after();
    } catch (e) { fail(e); after && after(); }
  }));
}
function habitChip(h) {
  const week = h.semana ? ` ${h.semana.hechas}/${h.semana.meta}` : "";
  const streak = h.racha > 0 ? ` <span class="streak">${I.flame}${h.racha}</span>` : "";
  return `<button class="chip ${h.hecho_hoy ? "on" : ""}" data-habit="${esc(h.id)}">${h.hecho_hoy ? I.check : ""}${esc(h.nombre)}${week}${streak}</button>`;
}
function wireHabits(root, after) {
  root.querySelectorAll("[data-habit]").forEach((b) => b.addEventListener("click", async () => {
    b.disabled = true;
    try { await api("/movil/habito", { id: b.dataset.habit, dia: b.dataset.day || today() }); } catch (e) { fail(e); }
    after && after();
  }));
}

// ---------- Hoy ----------
async function drawToday() {
  setTitle(longDate(today()).replace(/^./, (c) => c.toUpperCase()));
  await load("hoy", `/movil/hoy?hoy=${today()}`, (d) => {
    const now = d.hoy;
    const tomorrow = addDays(now, 1);
    const tasks = d.tareas || [];
    const late = tasks.filter((x) => x.fecha < now);
    const forToday = tasks.filter((x) => x.fecha === now);
    const forTomorrow = tasks.filter((x) => x.fecha === tomorrow);
    const week = tasks.filter((x) => x.fecha > tomorrow);
    const ev = (f) => (d.eventos || []).filter(f);
    let h = `<div class="capture">
      <textarea id="jot" rows="2" placeholder="${esc(t("Anota algo…"))}"></textarea>
      <div class="row"><button class="btn primary" id="jot-save">${esc(t("Anotar"))}</button><button class="btn" id="jot-task">${esc(t("Como tarea"))}</button></div>
    </div>`;
    if ((d.por_agendar || []).length) {
      h += `<h2>${esc(t("Por agendar"))}</h2>`;
      for (const p of d.por_agendar) {
        const e = p.evento;
        h += `<div class="card proposal"><div class="meta">${esc(whenText(e.fecha))}${e.hora ? " · " + esc(e.hora) : ""}</div>
          <div class="text"><b>${esc(e.titulo)}</b></div>
          <div class="meta">${esc(t("Llegó por correo: no está en tu agenda hasta que la agendes."))}</div>
          <div class="actions"><button class="btn small primary" data-decide="${esc(p.clave)}" data-yes="1">${esc(t("Agendar"))}</button><button class="btn small" data-decide="${esc(p.clave)}">${esc(t("Descartar"))}</button></div></div>`;
      }
    }
    if ((d.habitos || []).length) {
      const m = d.meta || {};
      const goal = m.total === 0 ? t("Hoy no toca ninguno de todos los días") : m.cumplida ? `${t("Meta de hoy cumplida")}${m.racha > 1 ? ` · ${m.racha} ${t("días")}` : ""}` : `${m.hechos}/${m.total}`;
      h += `<h2>${esc(t("Hábitos"))} <span class="goal ${m.cumplida ? "ok" : ""}">· ${esc(goal)}</span></h2><div class="chips">${d.habitos.map(habitChip).join("")}</div>`;
    }
    const section = (title, ts, es, cls) => {
      if (!ts.length && !es.length) return "";
      return `<h2 class="${cls || ""}">${esc(title)}</h2><div class="list">${es.map((e) => eventItem(e, title === t("Esta semana"))).join("")}${ts.map((x) => taskItem(x, { noDate: title !== t("Atrasadas") && title !== t("Esta semana") })).join("")}</div>`;
    };
    const body =
      section(t("Atrasadas"), late, [], "warn") +
      section(t("Hoy"), forToday, ev((e) => e.fecha === now)) +
      section(t("Mañana"), forTomorrow, ev((e) => e.fecha === tomorrow)) +
      section(t("Esta semana"), week, ev((e) => e.fecha > tomorrow));
    h += body || `<p class="empty" style="margin-top:18px">${esc(t("Nada para hoy."))}</p>`;
    if (d.sin_fecha) h += `<p class="sub" style="margin-top:12px">${esc(t("Sin fecha"))}: ${d.sin_fecha} · <a href="#" id="see-tasks">${esc(t("Ver todo"))}</a></p>`;
    if ((d.nota_hoy || []).length) {
      h += `<details style="margin-top:14px"><summary>${esc(t("Nota de hoy"))} (${d.nota_hoy.length})</summary><div class="card note-text">${renderNote(d.nota_hoy.join("\n"))}</div></details>`;
    }
    view().innerHTML = h;
    const area = $("#jot");
    area.value = store.get("jot-draft", "");
    area.addEventListener("input", () => { store.set("jot-draft", area.value); area.style.height = "auto"; area.style.height = area.scrollHeight + "px"; });
    $("#jot-save").addEventListener("click", () => jot(false));
    $("#jot-task").addEventListener("click", () => jot(true));
    $("#see-tasks")?.addEventListener("click", (e) => { e.preventDefault(); tab = "tareas"; store.set("tab", tab); drawTabs(); show(); });
    wireTasks(view(), refreshSoon);
    wireHabits(view(), drawToday);
    view().querySelectorAll("[data-decide]").forEach((b) => b.addEventListener("click", async () => {
      b.disabled = true;
      try { await api("/movil/por-agendar", { clave: b.dataset.decide, agendar: b.dataset.yes === "1" }); } catch (e) { fail(e); }
      drawToday();
    }));
  });
}
async function jot(asTask) {
  const area = $("#jot");
  const text = area.value.trim();
  if (!text) { area.focus(); return; }
  try {
    if (asTask) await api("/movil/tarea", { hoy: today(), texto: text });
    else await api("/movil/anotar", { hoy: today(), texto: text });
    area.value = "";
    store.del("jot-draft");
    toast(asTask ? t("Tarea agregada") : t("Anotado en tu día: la IA lo organiza en tu computador"));
    drawToday();
  } catch (e) { fail(e); }
}
let refreshTimer;
function refreshSoon() { clearTimeout(refreshTimer); refreshTimer = setTimeout(show, 900); }

// ---------- Tareas ----------
async function drawTasks() {
  setTitle(t("Tareas"));
  await load("tareas", "/movil/tareas", (d) => {
    const now = today();
    const tomorrow = addDays(now, 1);
    const p = d.pendientes || [];
    const groups = [
      [t("Atrasadas"), p.filter((x) => x.fecha && x.fecha < now), "warn"],
      [t("Hoy"), p.filter((x) => x.fecha === now)],
      [t("Mañana"), p.filter((x) => x.fecha === tomorrow)],
      [t("Pendientes"), p.filter((x) => x.fecha && x.fecha > tomorrow)],
      [t("Sin fecha"), p.filter((x) => !x.fecha)],
    ];
    let h = `<div class="row"><input id="new-task" placeholder="${esc(t("Nueva tarea (p. ej. Llamar a Pedro el viernes)"))}" enterkeyhint="done"><button class="btn primary" id="add-task">${esc(t("Agregar"))}</button></div>`;
    for (const [title, ts, cls] of groups) {
      if (ts.length) h += `<h2 class="${cls || ""}">${esc(title)} · ${ts.length}</h2><div class="list">${ts.map((x) => taskItem(x)).join("")}</div>`;
    }
    if (!p.length) h += `<p class="empty" style="margin-top:16px">${esc(t("No hay tareas pendientes."))}</p>`;
    if ((d.hechas || []).length) h += `<details style="margin-top:16px"><summary>${esc(t("Hechas"))} (${d.hechas.length})</summary><div class="list">${d.hechas.map((x) => taskItem(x)).join("")}</div></details>`;
    view().innerHTML = h;
    const input = $("#new-task");
    const add = async () => {
      const text = input.value.trim();
      if (!text) return;
      try { await api("/movil/tarea", { hoy: today(), texto: text }); input.value = ""; toast(t("Tarea agregada")); drawTasks(); } catch (e) { fail(e); }
    };
    $("#add-task").addEventListener("click", add);
    input.addEventListener("keydown", (e) => { if (e.key === "Enter") add(); });
    wireTasks(view(), refreshSoon);
  });
}

// ---------- Agenda ----------
let agendaFrom = null;
async function drawAgenda() {
  setTitle(t("Agenda"));
  const from = agendaFrom || today();
  await load(`agenda-${from}`, `/movil/agenda?desde=${from}&dias=14`, (d) => {
    let h = `<div class="row" style="justify-content:space-between"><button class="btn small" id="prev">‹ ${esc(t("Semana anterior"))}</button><button class="btn small" id="next">${esc(t("Semana siguiente"))} ›</button></div>`;
    let any = false;
    for (let i = 0; i < 14; i++) {
      const day = addDays(d.desde, i);
      const es = (d.eventos || []).filter((e) => e.fecha === day);
      const ts = (d.tareas || []).filter((x) => x.fecha === day);
      if (!es.length && !ts.length) continue;
      any = true;
      h += `<h2>${esc(whenText(day))}${day === today() || day === addDays(today(), 1) ? " · " + esc(longDate(day)) : ""}</h2><div class="list">${es.map((e) => eventItem(e, false)).join("")}${ts.map((x) => taskItem(x, { noDate: true })).join("")}</div>`;
    }
    if (!any) h += `<p class="empty" style="margin-top:16px">${esc(t("Sin eventos ni tareas."))}</p>`;
    view().innerHTML = h;
    $("#prev").addEventListener("click", () => { agendaFrom = addDays(from, -7); drawAgenda(); });
    $("#next").addEventListener("click", () => { agendaFrom = addDays(from, 7); drawAgenda(); });
    wireTasks(view(), refreshSoon);
  });
}

// ---------- Hábitos ----------
async function drawHabits() {
  setTitle(t("Hábitos"));
  await load("hoy", `/movil/hoy?hoy=${today()}`, (d) => {
    const hs = d.todos_habitos || d.habitos || [];
    const m = d.meta || {};
    let h = "";
    if (!hs.length) {
      view().innerHTML = `<p class="empty">${esc(t("Aún no tienes hábitos: créalos en Notas, en el computador."))}</p>`;
      return;
    }
    const goal = m.total === 0 ? t("Hoy no toca ninguno de todos los días") : m.cumplida ? t("Meta de hoy cumplida") : `${t("Hoy")}: ${m.hechos} ${t("de")} ${m.total}`;
    h += `<p class="goal ${m.cumplida ? "ok" : ""}">${esc(goal)}${m.racha > 0 ? ` · ${t("racha")}: ${m.racha} ${esc(t("días"))}` : ""}</p>`;
    const now = today();
    // La cuadrícula: desde el lunes de hace 4 semanas.
    const dow = (parseDay(now).getDay() + 6) % 7;
    const first = addDays(now, -dow - 28);
    const wd = EN ? ["M", "T", "W", "T", "F", "S", "S"] : ["L", "M", "M", "J", "V", "S", "D"];
    for (const x of hs) {
      const done = new Set(x.hechos || []);
      const unit = t(x.unidad === "dias" ? "días" : x.unidad);
      const week = x.semana ? `${x.semana.hechas} ${t("de")} ${x.semana.meta} ${t("esta semana")}` : "";
      let grid = wd.map((w) => `<span class="wd">${w}</span>`).join("");
      for (let i = 0; addDays(first, i) <= now; i++) {
        const day = addDays(first, i);
        const dayDow = (parseDay(day).getDay() + 6) % 7;
        const counts = x.frecuencia.startsWith("dias:") ? x.frecuencia.slice(5).split(",").map(Number).includes(dayDow + 1) : true;
        const before = day < x.desde;
        const cls = [done.has(day) ? "on" : "", day === now ? "today" : "", !counts ? "off" : ""].join(" ");
        grid += counts && !before
          ? `<button class="${cls}" data-habit="${esc(x.id)}" data-day="${day}" aria-label="${esc(longDate(day))}"></button>`
          : `<button class="${cls} off" disabled aria-label="${esc(longDate(day))}"></button>`;
      }
      h += `<div class="card habit">
        <div class="head"><button class="check ${x.hecho_hoy ? "on" : ""}" data-habit="${esc(x.id)}" ${x.toca_hoy || x.semana ? "" : "disabled"}>${x.hecho_hoy ? I.check : ""}</button>
        <div class="name">${esc(x.nombre)}</div></div>
        <div class="meta">${[x.racha > 0 ? `<span class="streak">${esc(t("racha"))}: ${x.racha} ${esc(unit)}</span>` : "", esc(week)].filter(Boolean).join(" · ")}</div>
        <div class="grid">${grid}</div></div>`;
    }
    h += `<p class="sub">${esc(t("Para agregar hábitos o cambiarlos, usa Notas en el computador."))}</p>`;
    view().innerHTML = h;
    wireHabits(view(), drawHabits);
  });
}

// ---------- Notas ----------
let noteQuery = "";
let searchTimer;
async function drawNotes() {
  if (openNote) return drawNote(openNote);
  setTitle(t("Notas"));
  view().innerHTML = `<input type="search" id="q" placeholder="${esc(t("Buscar en tus notas"))}" value="${esc(noteQuery)}" enterkeyhint="search"><div id="results" style="margin-top:12px"></div>`;
  const q = $("#q");
  const run = () => load(`notas-${noteQuery}`, `/movil/notas?q=${encodeURIComponent(noteQuery)}`, (d) => {
    const list = d.notas || [];
    $("#results").innerHTML = list.length
      ? `<div class="list">${list.map((n) => `<div class="note-row" data-note="${esc(n.ruta)}"><div class="t">${esc(n.titulo)}</div><div class="meta">${esc(n.espacio)}</div>${n.vista ? `<div class="v">${esc(n.vista)}</div>` : ""}</div>`).join("")}</div>`
      : `<p class="empty">${esc(t("Ninguna nota tiene eso."))}</p>`;
    $("#results").querySelectorAll("[data-note]").forEach((el) => el.addEventListener("click", () => { openNote = el.dataset.note; drawNote(openNote); window.scrollTo(0, 0); }));
  });
  q.addEventListener("input", () => { noteQuery = q.value; clearTimeout(searchTimer); searchTimer = setTimeout(run, 350); });
  await run();
}

// Una nota, como se ve en la app (sin editarla: solo agregar líneas al final).
function renderNote(text) {
  const inline = (s) => {
    let h = esc(s);
    h = h.replace(/!\[([^\]]*)\]\([^)]*\)/g, (_, a) => `🖼 ${a || ""}`);
    h = h.replace(/\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g, (_, n, l) => `<b>${l || n}</b>`);
    h = h.replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, (_, l, u) => `<a href="${u}" target="_blank" rel="noopener">${l}</a>`);
    h = h.replace(/\[([^\]]+)\]\([^)]*\)/g, (_, l) => `📎 ${l}`);
    h = h.replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>");
    h = h.replace(/(^|\s)\^[a-z0-9]{4,8}(?=\s|$)/g, "");
    h = h.replace(/(^|\s)due:(\d{4}-\d{2}-\d{2})/g, (_, sp, d) => `${sp}<span class="meta">· ${esc(whenText(d))}</span>`);
    h = h.replace(/(^|\s)(#[\wáéíóúñü-]+)/gi, '$1<span class="tag">$2</span>');
    return h;
  };
  return text.split("\n").map((l) => {
    if (!l.trim()) return "<p></p>";
    const m = l.match(/^(\s*)(?:[-*] )?\[( |x|X)\] (.*)$/);
    if (m) {
      const done = m[2] !== " ";
      return `<p class="${m[1] ? "sub-line" : ""}"><span class="box">${done ? "☑" : "☐"}</span><span class="${done ? "done" : ""}">${inline(m[3])}</span></p>`;
    }
    const hd = l.match(/^#{1,4} (.*)$/);
    if (hd) return `<p class="h">${inline(hd[1])}</p>`;
    const sub = /^\s{2,}/.test(l);
    return `<p class="${sub ? "sub-line" : ""}">${inline(l.trim().replace(/^[-*] /, "• "))}</p>`;
  }).join("");
}

async function drawNote(ruta) {
  setTitle(ruta.replace(/\.md$/, "").split("/").pop());
  await load(`nota-${ruta}`, `/movil/nota?ruta=${encodeURIComponent(ruta)}`, (n) => {
    view().innerHTML = `<button class="btn small back" id="back">‹ ${esc(t("Volver"))}</button>
      <p class="sub">${esc(n.espacio)}</p>
      <div class="card note-text">${renderNote(n.texto || "")}</div>
      <div class="row" style="margin-top:10px"><input id="line" placeholder="${esc(t("Agregar una línea…"))}" enterkeyhint="send"><button class="btn primary" id="add-line">${esc(t("Agregar"))}</button></div>`;
    $("#back").addEventListener("click", () => { openNote = null; drawNotes(); });
    const input = $("#line");
    const add = async () => {
      const text = input.value.trim();
      if (!text) return;
      try { await api("/movil/nota/linea", { ruta, texto: text }); input.value = ""; toast(t("Agregado a la nota")); drawNote(ruta); } catch (e) { fail(e); }
    };
    $("#add-line").addEventListener("click", add);
    input.addEventListener("keydown", (e) => { if (e.key === "Enter") add(); });
  });
}

// ---------- IA ----------
let chat = store.get("chat", []);
async function drawAsk() {
  setTitle(t("IA"));
  const suggestions = ["¿Qué tengo hoy?", "¿Qué está atrasado?", "¿Qué tengo esta semana?"];
  let h = `<div class="chat" id="chat">`;
  if (!chat.length) {
    h += `<p class="sub">${esc(t("Pregúntale a la IA por tus notas, tareas o agenda"))}</p><div class="suggest">${suggestions.map((s) => `<button class="btn small" data-ask="${esc(t(s))}">${esc(t(s))}</button>`).join("")}</div>`;
  }
  for (const m of chat) h += `<div class="bubble ${m.role === "user" ? "me" : "ai"}">${esc(m.content)}</div>`;
  h += `</div>`;
  if (chat.length) h += `<p style="text-align:center"><button class="linkish" id="new-chat">${esc(t("Nueva conversación"))}</button></p>`;
  h += `<div style="height:70px"></div><div class="ask-bar"><div class="row"><input id="ask" placeholder="${esc(t("Pregúntale a la IA por tus notas, tareas o agenda"))}" enterkeyhint="send"><button class="btn primary" id="send" aria-label="Enviar">${I.send}</button></div></div>`;
  view().innerHTML = h;
  const input = $("#ask");
  const send = async (q) => {
    q = (q || input.value).trim();
    if (!q) return;
    input.value = "";
    const history = chat.slice(-12);
    chat.push({ role: "user", content: q });
    store.set("chat", chat);
    drawAsk();
    const wait = document.createElement("div");
    wait.className = "bubble ai wait";
    wait.textContent = t("Pensando…");
    $("#chat").appendChild(wait);
    window.scrollTo(0, document.body.scrollHeight);
    try {
      const r = await api("/movil/preguntar", { hoy: today(), pregunta: q, historia: history, idioma: EN ? "en" : "es" });
      chat.push({ role: "assistant", content: r.respuesta });
    } catch (e) {
      chat.push({ role: "assistant", content: e.message === "offline" ? t("Sin conexión: mostrando lo último que viste.") : e.message });
    }
    store.set("chat", chat.slice(-40));
    drawAsk();
  };
  $("#send").addEventListener("click", () => send());
  input.addEventListener("keydown", (e) => { if (e.key === "Enter") send(); });
  view().querySelectorAll("[data-ask]").forEach((b) => b.addEventListener("click", () => send(b.dataset.ask)));
  $("#new-chat")?.addEventListener("click", () => { chat = []; store.set("chat", chat); drawAsk(); });
  window.scrollTo(0, document.body.scrollHeight);
}

// ---------- Entrar ----------
function drawLogin(step, email, err) {
  $("#tabs").hidden = true;
  $("#top").hidden = true;
  const v = view();
  const head = `<img src="icon-192.png" alt=""><h1>Notas</h1><p class="sub">${esc(t("Entra con tu cuenta de Notas"))}</p>`;
  const error = err ? `<p class="error">${esc(err)}</p>` : "";
  if (step === "code") {
    v.innerHTML = `<div class="login">${head}<p class="sub">${esc(t("Te mandamos un código a"))} <b>${esc(email)}</b></p>
      <input id="code" inputmode="numeric" autocomplete="one-time-code" maxlength="6" placeholder="${esc(t("Código de 6 dígitos"))}">${error}
      <button class="btn primary" id="go">${esc(t("Entrar"))}</button><button class="linkish" id="other">${esc(t("Volver"))}</button></div>`;
    const go = async () => {
      try {
        const r = await api("/cuenta/entrar", { correo: email, codigo: $("#code").value.trim() });
        signedIn(r.token);
      } catch (e) { drawLogin("code", email, e.message === "offline" ? t("Sin conexión: mostrando lo último que viste.") : e.message); }
    };
    $("#go").addEventListener("click", go);
    $("#code").addEventListener("keydown", (e) => { if (e.key === "Enter") go(); });
    $("#other").addEventListener("click", () => drawLogin("email", email));
    $("#code").focus();
    return;
  }
  if (step === "password") {
    v.innerHTML = `<div class="login">${head}<input id="email" type="email" autocomplete="email" placeholder="${esc(t("Tu correo"))}" value="${esc(email || "")}">
      <input id="pass" type="password" autocomplete="current-password" placeholder="${esc(t("Tu clave"))}">${error}
      <button class="btn primary" id="go">${esc(t("Entrar"))}</button><button class="linkish" id="other">${esc(t("Usar un código por correo"))}</button></div>`;
    const go = async () => {
      const correo = $("#email").value.trim();
      try {
        const r = await api("/cuenta/entrar-clave", { correo, clave: $("#pass").value });
        signedIn(r.token);
      } catch (e) { drawLogin("password", correo, e.message === "offline" ? t("Sin conexión: mostrando lo último que viste.") : e.message); }
    };
    $("#go").addEventListener("click", go);
    $("#pass").addEventListener("keydown", (e) => { if (e.key === "Enter") go(); });
    $("#other").addEventListener("click", () => drawLogin("email", $("#email").value.trim()));
    return;
  }
  v.innerHTML = `<div class="login">${head}<input id="email" type="email" autocomplete="email" placeholder="${esc(t("Tu correo"))}" value="${esc(email || "")}">${error}
    <button class="btn primary" id="go">${esc(t("Mandarme un código"))}</button><button class="linkish" id="other">${esc(t("Usar mi clave"))}</button>
    <p class="sub" style="margin-top:18px">${esc(t("Tus notas se sincronizan con tu cuenta: lo que hagas aquí aparece en tus equipos."))}</p></div>`;
  const go = async () => {
    const correo = $("#email").value.trim();
    if (!correo) return;
    try {
      await api("/cuenta/codigo", { correo });
      drawLogin("code", correo);
    } catch (e) { drawLogin("email", correo, e.message === "offline" ? t("Sin conexión: mostrando lo último que viste.") : e.message); }
  };
  $("#go").addEventListener("click", go);
  $("#email").addEventListener("keydown", (e) => { if (e.key === "Enter") go(); });
  $("#other").addEventListener("click", () => drawLogin("password", $("#email").value.trim()));
}

function signedIn(tok) {
  token = tok;
  store.set("token", tok);
  start();
}

async function signOut(expired) {
  if (!expired && token) { try { await fetch(API + "/cuenta/salir", { method: "POST", headers: { Authorization: `Bearer ${token}` } }); } catch {} }
  token = "";
  // Lo guardado era de esta cuenta: se borra.
  try { localStorage.clear(); } catch {}
  chat = [];
  drawLogin("email", "", expired ? t("Tu sesión terminó: vuelve a entrar.") : "");
}

// ---------- Al instante: cuando algo cambia en otro equipo, se actualiza ----------
let seq = 0;
let watching = false;
async function watch() {
  if (watching) return;
  watching = true;
  while (token) {
    if (document.hidden) { await new Promise((r) => setTimeout(r, 3000)); continue; }
    try {
      const r = await fetch(`${API}/sync/esperar?desde=${seq}`, { headers: { Authorization: `Bearer ${token}` } });
      if (!r.ok) { await new Promise((res) => setTimeout(res, 10000)); continue; }
      const d = await r.json();
      if (seq && d.seq > seq && tab !== "ia" && !document.activeElement?.matches("input, textarea")) show();
      seq = d.seq;
    } catch {
      await new Promise((res) => setTimeout(res, 10000));
    }
  }
  watching = false;
}

function start() {
  $("#top").hidden = false;
  const r = $("#refresh");
  r.innerHTML = I.refresh;
  r.setAttribute("aria-label", t("Actualizar"));
  r.onclick = () => show();
  const out = $("#logout");
  out.innerHTML = I.logout;
  out.setAttribute("aria-label", t("Salir"));
  out.onclick = () => { if (confirm(`${t("Salir")}?`)) signOut(false); };
  drawTabs();
  show();
  watch();
}

document.addEventListener("visibilitychange", () => { if (!document.hidden && token) show(); });
if ("serviceWorker" in navigator) navigator.serviceWorker.register("sw.js").catch(() => {});
document.documentElement.lang = EN ? "en" : "es";
if (token) start(); else drawLogin("email", "");
