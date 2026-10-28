// Shared helpers and the app-wide state every page reads from.
export const { invoke } = window.__TAURI__.core;
export const { listen } = window.__TAURI__.event;

/** Chillarr inside Jellyfin's web pages (web/shim.js): runs as the signed-in Jellyfin user. */
export const WEB = !!window.__CHILLARR_WEB__;
/** Who that is (web_viewer), filled in at start. On the desktop app the PC's owner can do everything. */
export const viewer = { name: "", admin: !WEB, can_delete: !WEB };
/** Deleting files, stopping downloads, removing Watch now titles. */
export const canDelete = () => viewer.admin || viewer.can_delete;

export const $ = (sel, root = document) => root.querySelector(sel);

/** Small element builder. Text always goes in as text, never as HTML. */
export function el(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (v === undefined || v === null || v === false) continue;
    if (k === "class") e.className = v;
    else if (k === "text") e.textContent = v;
    else if (k === "dataset") Object.assign(e.dataset, v);
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat()) if (kid !== null && kid !== undefined && kid !== false) e.append(kid);
  return e;
}

const NS = "http://www.w3.org/2000/svg";
/** An outline icon from one or more path strings. */
export function icon(...paths) {
  const s = document.createElementNS(NS, "svg");
  s.setAttribute("viewBox", "0 0 24 24");
  s.setAttribute("aria-hidden", "true");
  for (const d of paths) {
    const p = document.createElementNS(NS, "path");
    p.setAttribute("d", d);
    s.append(p);
  }
  return s;
}
/** Whole-screen mode. `app` is what the user asked for (F11, the top bar
 *  button); the player borrows full screen on top of it and gives it back. */
export const screenMode = { app: false };
export async function windowFull(on) {
  try { return await invoke("window_fullscreen", { on }); } catch { return false; }
}

export const ICON = {
  play: "M8 5v14l11-7z",
  plus: "M12 5v14M5 12h14",
  info: "M12 8h.01M11 12h1v5h1",
  check: "M5 12l5 5 9-10",
  download: "M12 4v11M7 10l5 5 5-5M5 20h14",
  folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
  left: "M15 5l-7 7 7 7",
  right: "m9 5 7 7-7 7",
};

export const LABEL = {
  Prowlarr: "Prowlarr", Sonarr: "Sonarr", Radarr: "Radarr",
  FlareSolverr: "FlareSolverr", QBittorrent: "qBittorrent", Jellyfin: "Jellyfin",
};
export const ORDER = ["Prowlarr", "Sonarr", "Radarr", "FlareSolverr", "QBittorrent", "Jellyfin"];

let toastTimer;
export function toast(msg, bad = false) {
  const t = $("#toast");
  t.textContent = msg;
  t.classList.toggle("bad", bad);
  t.classList.remove("out");
  t.hidden = false;
  clearTimeout(toastTimer);
  // It slides away (styles.css .toast.out) before it is hidden.
  toastTimer = setTimeout(() => {
    t.classList.add("out");
    toastTimer = setTimeout(() => { t.hidden = true; t.classList.remove("out"); }, 250);
  }, 4200);
}

/** A themed yes/no question. Resolves true on yes. `no` names the other button. */
export function ask(title, body, items = [], yes = "Install", no = "Not now") {
  $("#ask-title").textContent = title;
  $("#ask-body").textContent = body;
  $("#ask-yes").textContent = yes;
  $("#ask button[value=no]").textContent = no;
  $("#ask-list").replaceChildren(...items.map((t) => el("li", { text: t })));
  const d = $("#ask");
  d.returnValue = "";
  if (d.open) { d.classList.remove("closing"); d.close(); }
  d.showModal();
  d.querySelectorAll("button[value]").forEach((b) => (b.onclick = (e) => { e.preventDefault(); d.returnValue = b.value; closeDialog(d); }));
  $("#ask-yes").focus();
  return new Promise((done) => d.addEventListener("close", () => done(d.returnValue === "yes"), { once: true }));
}

/** Closes a dialog with its fade-out instead of snapping shut. */
export function closeDialog(d) {
  if (!d.open || d.classList.contains("closing")) return;
  d.classList.add("closing");
  setTimeout(() => { d.classList.remove("closing"); d.close(); }, 200);
}

export function fmtTime(s) {
  s = Math.max(0, Math.floor(s || 0));
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = s % 60;
  const mm = h ? String(m).padStart(2, "0") : String(m);
  return `${h ? h + ":" : ""}${mm}:${String(sec).padStart(2, "0")}`;
}

/** Year · rating · kind, for heroes and details. */
export function metaBits(t) {
  const kinds = { movie: "Movie", series: "Series", episode: "Episode", anime: "Anime" };
  const bits = [];
  if (t.year) bits.push(el("span", { text: String(t.year) }));
  if (t.rating) bits.push(el("span", { class: "rating", text: `${(+t.rating).toFixed(1)} rating` }));
  bits.push(el("span", { text: kinds[t.kind] || "" }));
  if (t.subtitle) bits.push(el("span", { text: t.subtitle }));
  const out = [];
  bits.forEach((b, i) => { if (i) out.push(el("span", { class: "sep" })); out.push(b); });
  return out;
}

/* App-wide state, kept current by backend events. */
export const store = {
  cards: new Map(),
  logs: [],
  fixing: false,
  settings: null,
  subs: new Set(),
  emit(what) { for (const f of this.subs) f(what); },
  on(f) { this.subs.add(f); return () => this.subs.delete(f); },
};

listen("card", (e) => { store.cards.set(e.payload.id, e.payload); store.emit("cards"); });
listen("log", (e) => { store.logs.unshift(e.payload); store.emit("logs"); });
listen("fix-state", (e) => { store.fixing = e.payload; store.emit("fixing"); });
listen("indexers", (e) => { store.indexers = e.payload; store.emit("indexers"); });

/** "A, B and C". */
function joinNames(names) {
  return names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

// Fix everything asks before Defender's UAC prompt. Another question already
// on screen wins; the next Fix everything asks again. Only the No button is
// remembered as a no (Escape just closes it).
let defenderAsking = false;
listen("defender-ask", async (e) => {
  const names = e.payload || [];
  if (defenderAsking || !names.length || $("#ask").open) return;
  defenderAsking = true;
  try {
    const yes = await ask(`Let ${joinNames(names)} save into protected folders?`,
      "Defender's folder protection blocks them from saving to your library. Windows asks for admin next.", [], "Allow", "No");
    const answer = yes ? true : $("#ask").returnValue === "no" ? false : null;
    if (answer === null) return;
    if (!answer && store.settings) store.settings.defender_ask = false;
    await invoke("defender_answer", { yes: answer }).catch((err) => toast(String(err), true));
  } finally {
    defenderAsking = false;
  }
});
