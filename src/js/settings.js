// Settings: the six apps and Fix everything, indexers, library folders,
// app paths, the Jellyfin account and streaming, general options, logs.
import { hiddenBox } from "./hidden.js";
import { $, ask, audioChoice, canDelete, el, icon, ICON, invoke, LABEL, listen, ORDER, store, toast, viewer, WEB } from "./api.js";
import { startProfiles } from "./profiles.js";
import { installWithMeter, openSetup } from "./setup.js";
import { applyTheme, currentTheme, saveTheme, themePicker } from "./theme.js";
import { setSubScale, subScale } from "./subrender.js";
import { bytesText, deleteBookFile, deleteChapters } from "./read.js";

const SECTIONS = [
  ["system", "Status"],
  ["general", "General"],
  ["appearance", "Appearance"],
  ["library", "Library & storage"],
  ["account", "Account & devices"],
  ["indexers", "Download sites"],
  ["apps", "Apps"],
  ["logs", "Logs"],
];
// Listed under "Advanced": Chillarr looks after these by itself.
const ADVANCED = "indexers";
// In Jellyfin's web pages: only what a Jellyfin user may change from a browser.
const WEB_SECTIONS = () => [
  ["account", "Account"],
  ["general", "Playback"],
  ["remote", "Phone remote"],
  ...(viewer.admin ? [["library", "Library & storage"]] : []),
];
const BUSY = ["Starting...", "Testing...", "Testing indexers...", "Restarting...", "Installing...", "Switching to run as you...", "Updating..."];
const WHERE = {
  Prowlarr: "the official Prowlarr installer", Sonarr: "the official Sonarr installer", Radarr: "the official Radarr installer",
  QBittorrent: "the official qBittorrent installer", Jellyfin: "the official Jellyfin installer",
  FlareSolverr: "FlareSolverr from its official GitHub page, checks it, and puts it in your AppData folder",
};

let unsub = null;
let settingsSeq = 0;

/** Leaving Settings: its live lists stop listening. */
export function stopSettings() {
  settingsSeq++;
  if (unsub) unsub();
  unsub = null;
}

export function renderSettings(view, section = "system") {
  // Library and Storage were two pages; old links open the merged one.
  if (section === "storage") section = "library";
  const sections = WEB ? WEB_SECTIONS() : SECTIONS;
  if (!sections.some(([id]) => id === section)) section = sections[0][0];
  stopSettings();
  const nav = el("nav", { class: "settings-nav", "aria-label": "Settings" }, el("h1", { text: "Settings" }),
    sections.flatMap(([id, name]) => [
      !WEB && id === ADVANCED ? el("p", { class: "nav-label", text: "Advanced" }) : null,
      el("button", { class: id === section ? "on" : "", onclick: () => (location.hash = `settings/${id}`) }, name),
    ]));
  const panel = el("section", { class: "panel" });
  view.replaceChildren(el("div", { class: "settings" }, nav, panel));
  (WEB ? { account: webAccount, general: webGeneral, remote: webRemote, library } : { system, appearance, indexers, library, account, apps, general, logs })[section](panel);
}

/** Folds away what most people never need. */
const more = (title, ...kids) => el("details", { class: "more" }, el("summary", { text: title }), ...kids);

/* ---------- Status ---------- */
function appCard(c) {
  const busy = BUSY.includes(c.line);
  const acts = el("div", { class: "acts" });
  if (c.missing) acts.append(el("button", { class: "btn small primary", onclick: () => offerInstall([c.id]) }, "Install"));
  else if (c.update) acts.append(el("button", { class: "btn small ghost", onclick: () => offerUpdate(c) }, `Update to ${c.update}`));
  if (c.open_url && !c.missing) acts.append(el("button", { class: "link-btn", onclick: () => invoke("open_app", { id: c.id }) }, "Open"));
  return el("article", { class: busy ? "app-card busy" : "app-card", dataset: { level: c.level } },
    el("div", { class: "top" }, el("span", { class: "dot" }), el("h3", { text: LABEL[c.id] }), acts),
    el("p", { class: "line", text: c.line }),
    c.notes.length ? el("ul", { class: "notes" }, c.notes.map((n) => el("li", { text: n }))) : null);
}

function chain() {
  const link = (id) => el("div", { class: "link", dataset: { level: store.cards.get(id)?.level || "unknown" } }, el("span", { class: "dot" }), LABEL[id]);
  return el("div", { class: "chain", "aria-label": "How the apps connect" },
    link("FlareSolverr"), el("span", { class: "wire" }), link("Prowlarr"), el("span", { class: "wire" }),
    el("div", { class: "fork" }, link("Sonarr"), link("Radarr")), el("span", { class: "wire" }), link("QBittorrent"), el("span", { class: "wire" }), link("Jellyfin"));
}

let fixStep = null;
let fixDone = null;
const fixWatchers = new Set();
if (!WEB) {
  listen("fix-step", (e) => { fixStep = e.payload; fixWatchers.forEach((f) => f()); });
  listen("fix-done", (e) => { fixStep = null; fixDone = e.payload; fixWatchers.forEach((f) => f()); });
}

/** Where Fix everything is, then what it did. */
function fixProgress() {
  const box = el("div", { class: "fix-progress" });
  const draw = () => {
    if (fixStep) {
      box.replaceChildren(el("div", { class: "fix-line" }, el("span", { class: "spinner small" }), el("span", { text: fixStep.line }), el("small", { text: `Step ${fixStep.n} of ${fixStep.of}` })),
        el("div", { class: "meter-bar slim" }, el("span", { class: "meter-fill", style: `transform:scaleX(${fixStep.n / fixStep.of})` })));
    } else if (fixDone) {
      const { secs, bad, actions, needs } = fixDone;
      const ok = !bad;
      const head = ok ? (actions.length ? "Fixed." : "Everything already works.") : `${bad} still ${bad === 1 ? "needs" : "need"} attention. Their cards below say why.`;
      box.replaceChildren(el("div", { class: `fix-result ${ok ? "ok" : "bad"}` }, el("div", { class: "fix-line" }, el("strong", { text: head }), el("small", { text: `Took ${secs < 60 ? `${secs} s` : `${Math.round(secs / 60)} min`}` })),
        actions.length || needs.length ? el("ul", {}, [...actions, ...needs].map((a) => el("li", { text: a }))) : null));
    } else box.replaceChildren();
  };
  fixWatchers.add(draw);
  const off = new MutationObserver(() => { if (!box.isConnected) { fixWatchers.delete(draw); off.disconnect(); } });
  queueMicrotask(() => { if (box.parentNode) off.observe(box.getRootNode(), { childList: true, subtree: true }); });
  draw();
  return box;
}

function system(panel) {
  const fix = el("button", { class: "btn primary", onclick: () => { fixDone = null; invoke("fix_everything"); } }, "Fix everything");
  const summary = el("p", { class: "sub" });
  const cardsBox = el("div", { class: "apps" });
  const chainBox = el("div");
  const detail = more("Show each app", chainBox, cardsBox);
  let hadBad = false;
  const draw = () => {
    const list = ORDER.map((id) => store.cards.get(id)).filter(Boolean);
    const bad = list.filter((c) => c.level === "red" || c.level === "amber");
    // A new problem opens the list, so its reason is on screen. Closing it stays closed.
    if (bad.length && !hadBad) detail.open = true;
    hadBad = bad.length > 0;
    summary.textContent = store.fixing ? "Starting what's down and checking your download sites." : !list.length ? "Checking your apps..." : bad.length ? `${bad.length} of ${list.length} apps need attention. Press Fix everything.` : "Everything is working.";
    fix.disabled = store.fixing;
    fix.textContent = store.fixing ? "Fixing..." : "Fix everything";
    cardsBox.replaceChildren(...list.map((c, j) => { const n = appCard(c); n.style.setProperty("--j", j); if (cardsBox.dataset.drawn) n.style.animation = "none"; return n; }));
    cardsBox.dataset.drawn = "1";
    chainBox.replaceChildren(chain());
  };
  panel.append(el("div", { class: "panel-head" }, el("div", {}, el("h2", { text: "Status" }), summary), fix), fixProgress(), detail);
  draw();
  unsub = store.on((w) => { if (w === "cards" || w === "fixing") draw(); });
}

async function offerInstall(ids) {
  const one = ids.length === 1;
  const ok = await ask(one ? `Install ${LABEL[ids[0]]}?` : `Install ${ids.length} missing apps?`,
    one ? `Chillarr downloads ${WHERE[ids[0]]}. Windows asks for your OK once.` : "Chillarr downloads each one from its official source. Windows asks for your OK once.",
    one ? [] : ids.map((id) => LABEL[id]), "Install");
  if (!ok) return false;
  await installWithMeter(ids);
  return true;
}
export { offerInstall };

async function offerUpdate(c) {
  const ok = await ask(`Update ${LABEL[c.id]} to ${c.update}?`, `${LABEL[c.id]} closes for about a minute while it updates, then opens again. Downloads pause and pick up where they left off.`, [], "Update");
  if (ok) { try { await invoke("update_app", { id: c.id }); } catch (e) { toast(String(e), true); } }
}

/* ---------- Appearance ---------- */
function appearance(panel) {
  const s = () => currentTheme().surface;
  const seg = el("div", { class: "segmented", role: "group", "aria-label": "Background" });
  const drawSeg = () => seg.replaceChildren(...[["tinted", "Tinted"], ["black", "Pure black"]].map(([id, name]) =>
    el("button", { class: s() === id ? "on" : "", onclick: async () => { await saveTheme({ ...currentTheme(), surface: id }); drawSeg(); } }, name)));
  drawSeg();
  panel.append(el("h2", { text: "Appearance" }), el("p", { class: "sub", text: "Your colour goes on the whole app, the tray and your desktop and taskbar icons." }),
    el("div", { class: "box theme-box" },
      el("div", { class: "opt column" }, el("span", {}, el("b", { text: "Colour" }), el("small", { text: "Pick one, or your own. It changes as you drag." })), themePicker(() => toast("Colour saved. Your icons are updated."))),
      el("div", { class: "opt" }, el("span", {}, el("b", { text: "Background" }), el("small", { text: "Tinted leans toward your colour. Pure black is darkest on OLED screens." })), seg)));
}

/* ---------- Download sites ---------- */
const inMinutes = (s) => (s >= 3600 ? `${Math.round(s / 3600)} h` : `${Math.max(1, Math.round(s / 60))} min`);

/** The sites Chillarr adds by itself, and how each one stands. */
export async function suggestions(onAdded) {
  const info = await invoke("setup_indexers");
  const cards = (info.sites || []).map((r) => {
    const here = r.state === "added";
    const label = { added: "Added", waiting: "Try now", removed: "Add", soon: "Add" }[r.state];
    const btn = el("button", { class: here ? "btn small ghost" : "btn small primary", disabled: here }, label);
    btn.addEventListener("click", async () => {
      btn.disabled = true; btn.textContent = "Adding...";
      try { toast(await invoke("setup_add_indexer", { definition: r.def })); btn.textContent = "Added"; btn.className = "btn small ghost"; note.textContent = r.why; onAdded?.(); }
      catch (e) { toast(String(e), true); btn.disabled = false; btn.textContent = label; }
    });
    const line = r.state === "waiting" ? `Down right now. Trying again in ${inMinutes(r.next_in || 0)}.` : r.state === "removed" ? "You took this one out, so it stays out." : r.why;
    const note = el("span", { text: line, title: r.note || "" });
    const name = el("strong", { text: r.name }, r.books ? el("small", { class: "site-tag", text: "Books and comics" }) : null);
    return el("div", { class: "suggest-card", dataset: { state: r.state } }, el("div", {}, name, note), btn);
  });
  return el("div", {}, el("div", { class: "suggest" }, cards));
}

/** The manga sites the manga downloader has, for Settings (it may not be set up yet). */
async function mangaSites() {
  const box = el("div", { class: "manga-sites" }, el("p", { class: "sub", text: "Looking..." }));
  invoke("manga_sites").then((s) => {
    const more = s.other ? ` Plus ${s.other} in other languages.` : "";
    box.replaceChildren(el("p", { class: "sub", text: `${s.en.length} sites in English, all asked at once for every series. The sharpest copy wins.${more}` }),
      el("div", { class: "chips" }, s.en.map((n) => el("span", { class: "chip static", text: n }))));
  }).catch(() => box.replaceChildren(el("p", { class: "sub", text: "They're set up the first time you follow a series. Over 25 sites, so one going down never leaves a gap." })));
  return box;
}

async function indexers(panel) {
  const table = el("tbody");
  const test = el("button", { class: "btn ghost", onclick: async () => {
    test.disabled = true;
    test.textContent = "Testing...";
    try { draw(await invoke("test_indexers")); } catch (e) { toast(String(e), true); } finally { test.disabled = false; test.textContent = "Test all"; }
  } }, "Test all");
  const cell = (c) => {
    if (!c) return el("td", { class: "none", text: "Not used" });
    const td = el("td", { title: c.reason });
    td.append(el("span", { class: "dot", dataset: { level: c.level } }), c.level === "ok" ? "Working" : c.reason);
    return td;
  };
  // Books and comics are searched straight through Prowlarr, on the sites marked for books.
  let bookSites = new Set();
  const bookCell = (r) => {
    if (!bookSites.has(r.name)) return el("td", { class: "none", text: "Not used" });
    const c = r.prowlarr;
    if (!c) return el("td", { class: "none", text: "Not added" });
    return el("td", { title: c.reason }, el("span", { class: "dot", dataset: { level: c.level } }), c.level === "ok" ? "Searched" : c.reason);
  };
  const draw = (rows) => table.replaceChildren(...rows.map((r) => el("tr", {}, el("td", { text: r.name }), cell(r.prowlarr), cell(r.sonarr), cell(r.radarr), bookCell(r))));
  const sites = await invoke("setup_indexers").catch(() => ({ sites: [] }));
  bookSites = new Set((sites.sites || []).filter((s) => s.books).map((s) => s.name));
  panel.append(
    el("div", { class: "panel-head" }, el("div", {}, el("h2", { text: "Download sites" }), el("p", { class: "sub", text: "Where Chillarr looks for movies, shows, books and comics. It adds and checks these by itself." })), test),
    el("table", { class: "grid" }, el("thead", {}, el("tr", {}, ["Site", "Prowlarr", "Sonarr", "Radarr", "Books and comics"].map((h) => el("th", { text: h })))), table),
    el("h2", { text: "Sites Chillarr adds for you", style: "margin-top:36px;font-size:1.2rem" }),
    el("p", { class: "sub", text: "Each one is added by itself. A site that's down is tried again later until it works." }),
    await suggestions(async () => draw(await invoke("indexer_table"))),
    el("h2", { text: "Manga, manhwa and manhua sites", style: "margin-top:36px;font-size:1.2rem" }),
    await mangaSites()
  );
  draw(await invoke("indexer_table"));
}

/* ---------- Library ---------- */
/** Where qBittorrent saves downloads, with a button to move it. */
export function downloadsRow(onPick) {
  const path = el("p", { class: "path selectable", text: "..." });
  invoke("download_folder").then((p) => (path.textContent = p || "Not set")).catch(() => {});
  const change = el("button", { class: "btn small ghost" }, icon(ICON.folder), "Change");
  change.addEventListener("click", async () => {
    const p = await invoke("pick_folder");
    if (!p) return;
    if (onPick) { onPick(p); path.textContent = p; return; }
    change.disabled = true;
    try { await invoke("set_download_folder", { path: p }); path.textContent = p; toast("Downloads go there from now on."); }
    catch (e) { toast(String(e), true); }
    change.disabled = false;
  });
  return el("div", { class: "box", style: "margin:8px 0 26px" }, el("div", { class: "opt" },
    el("span", {}, el("b", { text: "Downloads folder" }), el("small", { text: "Where qBittorrent saves what it downloads, before it's sorted into your library." }), path), change));
}

export function folderPicker(label, hint, list) {
  const ul = el("ul", { class: "folder-list" });
  const draw = () => ul.replaceChildren(...list.map((p, i) => el("li", {}, icon(ICON.folder), el("span", { style: "flex:1", text: p }),
    el("button", { class: "link-btn", onclick: () => { list.splice(i, 1); draw(); } }, "Remove"))));
  const add = el("button", { class: "btn small ghost", onclick: async () => { const p = await invoke("pick_folder"); if (p && !list.includes(p)) { list.push(p); draw(); } } }, icon(ICON.plus), "Add folder");
  draw();
  return el("div", {}, el("h3", { text: label }), el("p", { text: hint }), ul, add);
}

function gb(bytes) {
  if (!bytes) return "0 GB";
  const g = bytes / 1024 ** 3;
  return g >= 1000 ? `${(g / 1024).toFixed(1)} TB` : `${g >= 10 ? Math.round(g) : g.toFixed(1)} GB`;
}

/* ---------- Library & storage ---------- */
const size = (b) => (b >= 1e12 ? `${(b / 1e12).toFixed(2)} TB` : `${(b / 1e9).toFixed(1)} GB`);
const CLEANUP = [[0, "Off"], [1, "After 1 day"], [3, "After 3 days"], [7, "After 7 days"], [14, "After 14 days"], [30, "After 30 days"], [60, "After 60 days"], [90, "After 90 days"]];
const subhead = (text, style = "") => el("h2", { text, style: `font-size:1.2rem;margin-top:32px;${style}` });

// In order: drives, folders, clean-up, what takes the most room, then the
// folder settings at the bottom.
async function library(panel) {
  panel.append(el("h2", { text: "Library & storage" }),
    el("p", { class: "sub", text: "See how full your drives are and clear out what you've watched." }));
  const drivesBox = el("div", {}, el("p", { class: "note", text: "Measuring..." }));
  const foldersBox = el("div");
  const roomBox = el("div");
  const settingsBox = el("div");
  panel.append(drivesBox, foldersBox, roomBox, readingBox(), settingsBox);
  const [o, stats, f] = await Promise.all([
    invoke("storage_overview").catch(() => null),
    invoke("library_stats").catch(() => []),
    WEB ? null : invoke("setup_folders").catch(() => ({ movies: [], series: [] })),
  ]);
  const haveDrives = !!o?.drives.length;
  drivesBox.replaceChildren(...(haveDrives ? [drivesList(o.drives)] : []));
  foldersBox.replaceChildren(...folderCards(stats, !haveDrives));
  roomBox.replaceChildren(...(o && (o.drives.length || o.items.length) ? storageParts(o)
    : [el("p", { class: "note", text: "Chillarr gets sizes from Radarr and Sonarr. Start them with Fix everything, then come back." })]));
  if (f) settingsBox.replaceChildren(...changeFolders(f));
}

/** Each drive the library sits on, and how full it is. */
function drivesList(list) {
  return el("div", { class: "box st-drives" }, list.map((d) => {
    const used = d.total - d.free;
    const pct = Math.round((used / d.total) * 100);
    return el("div", { class: "st-drive" + (pct >= 90 ? " full" : "") },
      el("div", { class: "st-drive-top" }, el("b", { text: d.path }), el("span", { text: `${size(d.free)} free of ${size(d.total)}` })),
      el("div", { class: "st-bar" }, el("span", { style: `width:${pct}%` })));
  }));
}

/** Movies, Series, Anime: what's in each, and Scan. `disks` gives each card
 * its own free-space bar, for when the drives list above couldn't be read. */
function folderCards(stats, disks) {
  const scan = el("button", { class: "btn small ghost" }, "Scan for new files");
  scan.addEventListener("click", async () => {
    scan.disabled = true;
    try { await invoke("library_scan"); toast("Jellyfin is scanning. New files show up in a minute."); } catch (e) { toast(String(e), true); }
    scan.disabled = false;
  });
  const box = el("div", { class: "stats" }, stats.map((f, j) => {
    const used = f.total ? 1 - f.free / f.total : 0;
    const card = el("article", { class: "stat-card" },
      el("div", { class: "head" }, el("span", { class: "dot", dataset: { level: f.accessible === false ? "red" : "ok" } }), el("h3", { text: f.name }), el("span", { class: "note", text: f.app })),
      el("p", { class: "path selectable", text: f.path }),
      el("div", { class: "nums" },
        el("div", {}, el("b", { text: String(f.have) }), el("span", { text: f.have === 1 ? "title downloaded" : "titles downloaded" })),
        el("div", {}, el("b", { text: String(Math.max(0, f.tracked - f.have)) }), el("span", { text: "waiting" })),
        el("div", {}, el("b", { text: gb(f.size) }), el("span", { text: "on disk" }))),
      disks ? el("div", { class: "disk" }, el("span", { style: `width:${Math.round(used * 100)}%` })) : null,
      disks ? el("div", { class: "disk-label" }, el("span", { text: `${gb(f.free)} free` }), el("span", { text: `of ${gb(f.total)}` })) : null,
      WEB ? null : el("div", { class: "acts" }, el("button", { class: "btn small ghost", onclick: () => invoke("open_folder", { path: f.path }) }, icon(ICON.folder), "Open folder")));
    card.style.setProperty("--j", j);
    return card;
  }));
  if (!stats.length) box.append(el("p", { class: "note", text: "No folders yet. Add one below." }));
  return [el("div", { class: "panel-head", style: "margin:32px 0 14px;align-items:center" }, subhead("Folders", "margin-top:0"), scan), box];
}

/** Clean up watched, Delete watched now, and what takes the most room. */
/** Manga, manhwa and books: where they go, and deleting chapters once read. */
function readingBox() {
  const path = el("p", { class: "path selectable", text: "..." });
  invoke("read_folder").then((p) => (path.textContent = p || "Picked next to your movies the first time you download something to read.")).catch(() => {});
  const change = el("button", { class: "btn small ghost" }, icon(ICON.folder), "Change");
  change.addEventListener("click", async () => {
    const p = await invoke("pick_folder");
    if (!p) return;
    change.disabled = true;
    try { toast(await invoke("read_set_folder", { path: p })); path.textContent = p; } catch (e) { toast(String(e), true); }
    change.disabled = false;
  });
  const days = el("select", { class: "quality", "aria-label": "Delete chapters after reading" },
    CLEANUP.map(([d, label]) => el("option", { value: String(d), text: label })));
  // As you read: each chapter goes once read. Finished: a series goes whole, once read to the end.
  let whole = false;
  const when = el("div", { class: "segmented", role: "group", "aria-label": "Delete when" });
  const drawWhen = () => when.replaceChildren(
    el("button", { class: whole ? "" : "on", onclick: () => setWhen(false) }, "As you read"),
    el("button", { class: whole ? "on" : "", onclick: () => setWhen(true) }, "When finished"));
  // Says exactly what each choice does, with the days filled in (#75).
  const whenText = el("small");
  const drawWhenText = () => {
    const n = +days.value || 0;
    const d = `${n} day${n === 1 ? "" : "s"}`;
    whenText.textContent = whole
      ? `When finished: nothing is deleted until everyone reading it has marked every downloaded chapter read. Then the whole series goes ${d} after the last one was marked. Marked read means you reached its last page, or marked it read yourself (one, a range or all).`
      : `As you read: each chapter is deleted ${d} after it was marked read. Marked read means you reached its last page, or marked it read yourself (one, a range or all).`;
  };
  const whenRow = el("div", { class: "opt" }, el("span", {}, el("b", { text: "Delete" }), whenText), when);
  const save = async (n) => {
    try {
      await invoke("read_set_cleanup", { days: n, whole });
      toast(!n ? "Chapters are kept." : whole ? `A series is deleted ${n} day${n === 1 ? "" : "s"} after everyone reading it has marked every downloaded chapter read.` : `Chapters are deleted ${n} day${n === 1 ? "" : "s"} after everyone reading them has read them.`);
    } catch (e) { toast(String(e), true); }
  };
  const setWhen = (w) => { whole = w; drawWhen(); drawWhenText(); save(+days.value || 0); };
  invoke("get_settings").then((s) => {
    days.value = String((s && s.read_cleanup_days) || 0);
    whole = !!(s && s.read_cleanup_whole);
    drawWhen();
    drawWhenText();
    whenRow.hidden = !(+days.value);
  }).catch(() => {});
  drawWhen();
  days.addEventListener("change", () => { const n = +days.value || 0; whenRow.hidden = !n; drawWhenText(); save(n); });
  const inside = el("div", { class: "rs-inside" });
  const room = el("div", { class: "rs-room" });
  drawReadingRoom(inside, room);
  return el("div", {}, subhead("Reading"),
    el("div", { class: "box" },
      el("div", { class: "opt" }, el("span", {}, el("b", { text: "Reading folder" }), el("small", { text: "Manga, manhwa, manhua and books go here." }), path), change),
      inside,
      el("div", { class: "opt" }, el("span", {}, el("b", { text: "Delete chapters after reading" }),
        el("small", { text: "Frees the space after reading, like films after watching. A chapter someone else hasn't reached yet stays. Deleted chapters can be downloaded again." })), days),
      whenRow),
    room);
}

/* What's inside the reading folder, and what in it takes the most room (#39). */
const READ_FOLDER_NOTE = {
  Comics: "Manga, manhwa and manhua chapters, a folder per site and series.",
  Books: "Books and light novels.",
  Sharpened: "Pages your graphics card sharpened.",
};
async function drawReadingRoom(inside, room) {
  let o;
  try { o = await invoke("read_storage"); } catch (e) { return; }
  if (!o || !o.folders.length) return;
  const all = o.folders.find((f) => f.name === "Reading");
  inside.replaceChildren(...o.folders.filter((f) => f.name !== "Reading").map((f) => el("div", { class: "opt rs-folder" },
    el("span", {}, el("b", { text: `${f.name} · ${bytesText(f.bytes)}` }), el("small", { text: READ_FOLDER_NOTE[f.name] || "" }), el("p", { class: "path selectable", text: f.path })),
    WEB || !f.exists ? null : el("button", { class: "btn small ghost", onclick: () => invoke("open_folder", { path: f.path }).catch((e) => toast(String(e), true)) }, icon(ICON.folder), "Open folder"))),
    el("p", { class: "note rs-total", text: `Reading takes ${bytesText(all ? all.bytes : 0)} in all: ${o.chapters} chapter${o.chapters === 1 ? "" : "s"}, ${o.books.length} book${o.books.length === 1 ? "" : "s"}, ${bytesText(o.sharp_bytes)} of sharpened copies.${o.total ? ` ${size(o.free)} free on this drive.` : ""}` }));
  const items = [
    ...o.series.map((x) => ({ kind: "series", x, bytes: x.bytes })),
    ...o.books.map((x) => ({ kind: "book", x, bytes: x.bytes })),
  ].sort((a, b) => b.bytes - a.bytes);
  if (!items.length) { room.replaceChildren(); return; }
  const biggest = items[0].bytes || 1;
  const redraw = () => { const r = $("#view"); if (r) renderSettings(r, "library"); };
  const list = el("ul", { class: "st-list" }, items.map(({ kind, x, bytes }) => {
    const del = el("button", { class: "btn small ghost danger" }, "Delete");
    del.addEventListener("click", async () => {
      del.disabled = true;
      const done = kind === "series" ? await deleteChapters(x.key, null) : await deleteBookFile(x, x.others || []);
      if (done) redraw(); else del.disabled = false;
    });
    const unread = kind === "series" ? x.chapters - x.read : 0;
    const what = kind === "series"
      ? `${x.chapters} chapter${x.chapters === 1 ? "" : "s"} · ${x.read} read, ${unread} unread`
      : x.author || "Book";
    const whose = x.others && x.others.length ? `${x.mine ? "Also read by" : "Read by"} ${x.others.join(", ")}` : null;
    return el("li", {},
      el("div", { class: "st-thumb", style: x.cover ? `background-image:url("${String(x.cover).replace(/"/g, "%22")}")` : "" }),
      el("div", { class: "st-info" },
        el("b", { text: x.title }),
        el("small", { text: [what, x.sharp_bytes ? `${bytesText(x.sharp_bytes)} sharpened` : null, whose].filter(Boolean).join(" · ") }),
        el("div", { class: "st-bar thin" }, el("span", { style: `width:${Math.max(2, Math.round((bytes / biggest) * 100))}%` }))),
      el("span", { class: "st-size", text: bytesText(bytes) }), canDel() ? del : el("span"));
  }));
  room.replaceChildren(subhead("What reading takes"), list);
}
const canDel = () => canDelete();

/** Deletes these titles all at once (a few at a time, so the PC isn't
    swamped), each gone from `o.items` as soon as it's asked for. Permanent:
    Radarr and Sonarr have no recycle bin set, and nothing goes to Windows'. */
async function deleteMany(o, items, onEach) {
  const gone = new Set(items);
  o.items = o.items.filter((i) => !gone.has(i));
  onEach();
  const failed = [];
  const queue = [...items];
  const worker = async () => {
    for (let i = queue.shift(); i; i = queue.shift()) {
      try { await invoke("media_delete", { tile: i.tile }); } catch (e) { failed.push(`${i.tile.title}: ${e}`); }
    }
  };
  await Promise.all(Array.from({ length: Math.min(4, items.length) }, worker));
  const freed = items.reduce((n, i) => n + i.size, 0);
  if (failed.length) toast(`Couldn't delete ${failed.length}: ${failed.join("; ")}`, true);
  else toast(items.length === 1 ? `Deleted ${items[0].tile.title}. Freed ${size(freed)}.` : `Deleted ${items.length} titles. Freed ${size(freed)}.`);
  if (failed.length) renderSettings($("#view"), "library");
}

function storageParts(o) {
  let filter = "all";
  const picked = new Set();
  const total = o.items.reduce((n, i) => n + i.size, 0);
  const watched = o.items.filter((i) => i.watched);
  const watchedSize = watched.reduce((n, i) => n + i.size, 0);

  const cleanup = el("select", { class: "quality", "aria-label": "Clean up watched" },
    CLEANUP.map(([d, label]) => el("option", { value: String(d), text: label, selected: (o.cleanup_days || 0) === d })));
  cleanup.addEventListener("change", async () => {
    const days = +cleanup.value || null;
    try { await invoke("storage_set_cleanup", { days }); toast(days ? `Each episode or movie is deleted ${days} day${days === 1 ? "" : "s"} after you watch it.` : "Clean-up is off."); }
    catch (e) { toast(String(e), true); }
  });
  const sweep = el("button", { class: "btn small ghost", disabled: !watched.length }, watched.length ? `Delete watched now (${size(watchedSize)})` : "Nothing watched to delete");
  sweep.addEventListener("click", async () => {
    const ok = await ask(`Delete ${watched.length} watched title${watched.length === 1 ? "" : "s"}?`, `This frees ${size(watchedSize)}. The files are deleted for good (not to the Recycle Bin); you can always download them again.`, watched.map((i) => `${i.tile.title} (${size(i.size)})`), "Delete");
    if (!ok) return;
    sweep.disabled = true;
    sweep.textContent = "Nothing watched to delete";
    await deleteMany(o, watched, draw);
  });
  const tidy = el("div", { class: "box" },
    el("div", { class: "opt" }, el("span", {}, el("b", { text: "Clean up watched" }),
      el("small", { text: "Deletes episodes and movies you've finished after this many days, and they won't download again. Anything another profile is still watching stays." })), cleanup),
    el("div", { class: "opt" }, el("span", {}, el("b", { text: `${o.items.length} titles, ${size(total)}` }),
      el("small", { text: `${watched.length} watched (${size(watchedSize)}).` })), canDel() ? sweep : null));

  const chips = el("div", { class: "st-chips" });
  const list = el("ul", { class: "st-list" });
  const bar = el("div", { class: "st-selbar" });
  const biggest = o.items[0]?.size || 1;
  const draw = () => {
    for (const i of [...picked]) if (!o.items.includes(i)) picked.delete(i);
    chips.replaceChildren(...[["all", "All"], ["watched", "Watched"], ["unwatched", "Not watched"]].map(([id, label]) =>
      el("button", { class: id === filter ? "chip on" : "chip", onclick: () => { filter = id; draw(); } }, label)));
    const shown = o.items.filter((i) => filter === "all" || (filter === "watched") === i.watched);
    // Ticking several: one Delete for all of them.
    if (canDel() && shown.length) {
      const all = shown.every((i) => picked.has(i));
      const sel = [...picked];
      const go = el("button", { class: "btn small danger-fill", disabled: !sel.length }, sel.length ? `Delete ${sel.length} (${size(sel.reduce((n, i) => n + i.size, 0))})` : "Delete selected");
      go.addEventListener("click", async () => {
        const ok = await ask(`Delete ${sel.length} title${sel.length === 1 ? "" : "s"}?`, "They're deleted from your PC for good (not to the Recycle Bin). You can always download them again.", sel.map((i) => `${i.tile.title} (${size(i.size)})`), "Delete");
        if (!ok) return;
        picked.clear();
        await deleteMany(o, sel, draw);
      });
      bar.replaceChildren(
        el("label", { class: "st-pick" }, el("input", { type: "checkbox", checked: all, onchange: (e) => { for (const i of shown) { if (e.target.checked) picked.add(i); else picked.delete(i); } draw(); } }), el("span", { text: "Select all" })),
        go);
    } else bar.replaceChildren();
    list.replaceChildren(...shown.map((i) => {
      const del = el("button", { class: "btn small ghost danger" }, "Delete");
      del.addEventListener("click", async () => {
        const ok = await ask(`Delete ${i.tile.title}?`, `This deletes it from your PC for good (not to the Recycle Bin) and frees ${size(i.size)}. You can always download it again.`, [], "Delete");
        if (!ok) return;
        await deleteMany(o, [i], draw);
      });
      const tick = canDel() ? el("input", { type: "checkbox", class: "st-tick", "aria-label": `Select ${i.tile.title}`, checked: picked.has(i), onchange: (e) => { if (e.target.checked) picked.add(i); else picked.delete(i); draw(); } }) : null;
      const kind = i.tile.kind === "series" ? "Series" : "Movie";
      return el("li", {}, tick,
        el("div", { class: "st-thumb", style: i.tile.poster ? `background-image:url("${i.tile.poster.replace(/"/g, "%22")}")` : "" }),
        el("div", { class: "st-info" },
          el("b", { text: i.tile.title }),
          el("small", { text: [kind, i.tile.year, i.watched ? "Watched" : null].filter(Boolean).join(" · ") }),
          el("div", { class: "st-bar thin" }, el("span", { style: `width:${Math.max(2, Math.round((i.size / biggest) * 100))}%` }))),
        el("span", { class: "st-size", text: size(i.size) }), canDel() ? del : null);
    }));
    if (!shown.length) list.append(el("li", { class: "st-empty" }, filter === "watched" ? "Nothing watched yet." : "Nothing here."));
  };
  draw();
  return [tidy, subhead("What takes the most room"), chips, bar, list];
}

/** The Downloads folder and the Movies / Series folder pickers, with Save. */
function changeFolders(f) {
  const movies = [...f.movies], series = [...f.series];
  const save = el("button", { class: "btn primary" }, "Save folders");
  const err = el("p", { class: "err" });
  save.addEventListener("click", async () => {
    save.disabled = true; err.textContent = "";
    try { await invoke("setup_set_folders", { movies, series }); toast("Folders saved. Jellyfin is scanning them."); renderSettings($("#view"), "library"); }
    catch (e) { err.textContent = String(e); }
    save.disabled = false;
  });
  return [more("Change folders",
    downloadsRow(),
    el("p", { class: "note", text: "A folder with \"Anime\" in its name is used for anime. Removing a folder here doesn't delete anything." }),
    el("div", { class: "folder-step", style: "margin-top:16px" },
      folderPicker("Movies", "New movies go here.", movies),
      folderPicker("Series and anime", "New series go here. Add a separate folder named Anime for anime.", series)),
    err, el("div", { style: "margin-top:18px" }, save))];
}

/* ---------- Account & devices ---------- */
/** What apps_set_login did, as one toast line. */
export function loginSummary(outcomes, lead = "") {
  const set = outcomes.filter((o) => o.result === "set").map((o) => o.app);
  const later = outcomes.filter((o) => o.result === "later").map((o) => o.app);
  const parts = [lead];
  if (set.length) parts.push(`${set.join(", ")} now use${set.length === 1 ? "s" : ""} this login.`);
  if (later.length) parts.push(`${later.join(", ")} get${later.length === 1 ? "s" : ""} it on the next Fix everything.`);
  return parts.filter(Boolean).join(" ");
}

/** One login for the download apps, changeable any time. Jellyfin keeps its own password. */
function appsLoginBlock(name) {
  const user = el("input", { type: "text", autocomplete: "username", value: name || "" });
  const pass = el("input", { type: "password", autocomplete: "new-password" });
  const err = el("p", { class: "err" });
  const go = el("button", { class: "btn small primary" }, "Set login");
  go.addEventListener("click", async () => {
    go.disabled = true; err.textContent = "";
    try { toast(loginSummary(await invoke("apps_set_login", { user: user.value, password: pass.value }))); pass.value = ""; }
    catch (e) { err.textContent = String(e); }
    go.disabled = false;
  });
  return el("div", { class: "box", style: "margin-top:24px" },
    el("div", { class: "opt" }, el("span", {}, el("b", { text: "One login for your apps" }),
      el("small", { text: "qBittorrent, Sonarr, Radarr and Prowlarr ask for this name and password when opened in a browser. Programs on this PC, Chillarr included, skip it. Chillarr doesn't keep the password." }))),
    el("div", { style: "display:grid;grid-template-columns:1fr 1fr auto;gap:12px;align-items:end;padding:0 18px 16px" },
      el("label", { class: "field" }, el("small", { text: "Name" }), user),
      el("label", { class: "field" }, el("small", { text: "Password" }), pass), go),
    err);
}

async function account(panel) {
  const ms = await invoke("media_status");
  panel.append(el("h2", { text: "Account & devices" }),
    el("p", { class: "sub", text: "Your profiles, and how to watch on your phone, TV or laptop. Nothing goes through the cloud: your PC does the work." }),
    hiddenBox());
  if (!ms.installed) {
    panel.append(el("button", { class: "btn primary", onclick: () => offerInstall(["Jellyfin"]) }, "Install Jellyfin"));
    return;
  }
  if (!ms.up) {
    panel.append(el("p", { class: "note", text: "Jellyfin isn't answering. Open System and press Fix everything." }));
    return;
  }
  if (ms.signed_in) {
    panel.append(el("div", { class: "box" },
      el("div", { class: "opt" }, el("span", {}, el("b", { text: `Signed in as ${ms.user}` })),
        el("button", { class: "btn small ghost", onclick: async () => { await invoke("media_sign_out"); renderSettings($("#view"), "account"); window.dispatchEvent(new Event("chillarr:refresh")); } }, "Sign out")),
      el("div", { class: "opt" }, el("span", {}, el("b", { text: "Profiles" }),
        el("small", { text: "Everyone gets their own Continue watching and My List. Add a PIN to keep one private." })),
        el("button", { class: "btn small ghost", onclick: () => window.dispatchEvent(new Event("chillarr:profiles")) }, "Manage profiles"))));
  } else {
    const user = el("input", { type: "text", autocomplete: "username", value: "" });
    const pass = el("input", { type: "password", autocomplete: ms.setup_done ? "current-password" : "new-password" });
    const err = el("p", { class: "err" });
    const go = el("button", { class: "btn primary" }, ms.setup_done ? "Sign in" : "Create account");
    go.addEventListener("click", async () => {
      if (!user.value.trim() || !pass.value) { err.textContent = "Enter a name and a password."; return; }
      go.disabled = true; err.textContent = "";
      try {
        await invoke("media_connect", { user: user.value.trim(), password: pass.value });
        toast(loginSummary(await invoke("apps_set_login", { user: user.value.trim(), password: pass.value }).catch(() => []), "Jellyfin is ready."));
        renderSettings($("#view"), "account");
        window.dispatchEvent(new Event("chillarr:refresh"));
        // Still locked (a PIN, or the main profile's password): ask who's watching.
        startProfiles().catch(() => {});
      }
      catch (e) { err.textContent = String(e); go.disabled = false; }
    });
    panel.append(el("p", { class: "note", text: ms.setup_done ? "Sign in with your Jellyfin account. qBittorrent, Sonarr, Radarr and Prowlarr get the same login." : "Jellyfin is new. Pick a name and password; Chillarr sets it up with your library folders, and qBittorrent, Sonarr, Radarr and Prowlarr get the same login." }),
      el("div", { style: "display:grid;gap:14px;max-width:420px;margin-top:14px" },
        el("label", { class: "field" }, el("small", { text: "Name" }), user), el("label", { class: "field" }, el("small", { text: "Password" }), pass), el("div", {}, go), err));
  }
  panel.append(el("h2", { text: "Watch on other devices", style: "margin-top:36px;font-size:1.2rem" }),
    el("p", { class: "note", text: "Install the free Jellyfin app on your phone or TV. On the same Wi-Fi, connect it to this address:" }),
    el("p", { class: "addr", text: ms.address || "http://this-pc:8096" }),
    el("h2", { text: "Watch away from home", style: "margin-top:30px;font-size:1.2rem" }),
    ms.tailscale || ms.tailscale_ip
      ? el("div", {},
        el("p", { class: "note", text: "Tailscale is connected on this PC. Install Tailscale on your phone or laptop, sign in with the same account, then open the Jellyfin app and connect to:" }),
        el("p", { class: "addr selectable", text: ms.tailscale || ms.tailscale_ip }),
        ms.tailscale && ms.tailscale_ip ? el("p", { class: "note", text: `If that name doesn't work in an app, use ${ms.tailscale_ip}` }) : null,
        el("p", { class: "note", text: "It works anywhere with internet, stays private (only your own devices can reach it), and costs nothing. This PC has to be on with Chillarr running." }))
      : el("div", { class: "tip" }, el("span", { text: "Install Tailscale (free, tailscale.com) on this PC and on your phone, and sign in with the same account. Your Tailscale address shows up here, ready for the Jellyfin app, wherever you are." })));
  if (ms.signed_in) {
    panel.append(await subtitleBlock(ms.user),
      more("More options", shelfBlock(), searchBlock(), appsLoginBlock(ms.user),
        el("div", { class: "box", style: "margin-top:24px" }, el("div", { class: "opt" }, el("span", {}, el("b", { text: "Browsers that know the main profile" }),
          el("small", { text: "They'll ask for its password again. Useful after a lost phone or a lent laptop." })),
          el("button", { class: "btn small ghost", onclick: async (e) => {
            const b = e.currentTarget;
            b.disabled = true;
            try { await invoke("profiles_forget_web_devices"); toast("Every browser asks for the main profile's password again."); }
            catch (err) { toast(String(err), true); }
            b.disabled = false;
          } }, "Forget browsers")))));
  }
}

/** Smarter search in Jellyfin's own apps (meili.rs). */
function searchBlock() {
  const box = el("div", { class: "box", style: "margin-top:24px" });
  const draw = (st, busy = "") => {
    const sw = el("input", { type: "checkbox", class: "switch", disabled: !!busy });
    sw.checked = busy ? !st.on : st.on;
    const line = busy || (!st.on ? "Off. Jellyfin only finds titles typed exactly as they're written."
      : st.connected ? `On. Knows ${st.items != null ? st.items.toLocaleString() + " titles and episodes" : "your library"}.`
      : st.running ? "On, but Jellyfin hasn't connected to it yet. It does after Jellyfin restarts."
      : "On, but its engine isn't running. Chillarr starts it again within a minute.");
    sw.addEventListener("input", async () => {
      const on = sw.checked;
      draw(st, on ? "Turning on: downloading the search engine (129 MB), then Jellyfin restarts once..." : "Turning off: Jellyfin restarts once...");
      try { toast(await invoke("meili_set", { on })); } catch (e) { toast(String(e), true); }
      draw(await invoke("meili_status").catch(() => st));
    });
    box.replaceChildren(el("label", { class: "opt" }, el("span", {}, el("b", { text: "Smarter search" }),
      el("small", { text: "Lets search on your TV and phone forgive typos, so \"spiderman\" finds Spider-Man." }),
      el("small", { class: "search-state", text: line })), sw));
  };
  invoke("meili_status").then((st) => draw(st)).catch(() => {});
  return box;
}

/** The Discover shelf and Chillarr's name in Jellyfin apps on TVs and phones. */
function shelfBlock() {
  const box = el("div", { class: "shelf-block" }, el("h2", { text: "TV and Jellyfin apps", style: "margin-top:36px;font-size:1.2rem" }));
  const draw = (st) => {
    // A change that fails puts everything back as it was, and says why.
    const change = async (input, call) => {
      if (input) input.disabled = true;
      try { draw(await call); } catch (e) { toast(String(e), true); draw(st); }
    };
    const on = el("input", { type: "checkbox", class: "switch" });
    on.checked = st.on;
    on.addEventListener("input", () => change(on, invoke("shelf_set", { on: on.checked, seriesMode: null })));
    const brand = el("input", { type: "checkbox", class: "switch" });
    brand.checked = st.brand;
    brand.addEventListener("input", () => change(brand, invoke("shelf_brand", { on: brand.checked })));
    const seg = el("div", { class: "segmented", role: "group", "aria-label": "When a show is favourited" },
      [["season1", "Season 1"], ["all", "Whole show"]].map(([v, t]) => el("button", { class: st.series_mode === v ? "on" : "", onclick: () => change(null, invoke("shelf_set", { on: st.on, seriesMode: v })) }, t)));
    const count = st.on
      ? `${st.movies} movies and ${st.series} series on the shelf${st.requested.length ? `. On their way: ${st.requested.slice(0, 4).join(", ")}${st.requested.length > 4 ? ` and ${st.requested.length - 4} more` : ""}.` : "."}`
      : "Off.";
    box.replaceChildren(box.firstChild,
      el("p", { class: "note", text: "Get new titles from a TV or phone: open Discover Movies or Discover Series in the Jellyfin app and press the heart on anything. This PC downloads it." }),
      el("div", { class: "box" },
        el("label", { class: "opt" }, el("span", {}, el("b", { text: "Discover shelf" }), el("small", { text: count })), on),
        el("div", { class: "opt" }, el("span", {}, el("b", { text: "When a show is favourited" }), el("small", { text: "Get the first season, or every season. A heart on one episode gets just that episode." })), seg),
        el("label", { class: "opt" }, el("span", {}, el("b", { text: "Show as Chillarr" }), el("small", { text: "Jellyfin apps call this server Chillarr, with its colours and a short welcome on the sign-in screen." })), brand)));
  };
  invoke("shelf_status").then(draw).catch(() => {});
  return box;
}

const SUBS_PITCH = "Optional. A free OpenSubtitles account finds subtitles for almost anything, in any language. Without one, Chillarr still finds them from free sources, just fewer.";

/** The OpenSubtitles.com sign-in. `name` is the one login from setup: with the
 *  same name on both, only the password is left to type. */
function subtitleForm(name, installed, onDone) {
  const user = el("input", { type: "text", autocomplete: "username", value: name || "" });
  const pass = el("input", { type: "password", autocomplete: "current-password" });
  const err = el("p", { class: "err" });
  const go = el("button", { class: "btn primary", type: "button" }, "Turn on");
  go.addEventListener("click", async () => {
    if (!user.value.trim() || !pass.value) { err.textContent = "Enter your OpenSubtitles name and password."; return; }
    go.disabled = true; err.textContent = "";
    go.textContent = installed ? "Checking..." : "Setting up, about a minute...";
    try { toast(await invoke("subs_setup", { username: user.value.trim(), password: pass.value })); onDone(); }
    catch (e) { err.textContent = String(e); go.disabled = false; go.textContent = "Turn on"; }
  });
  const signUp = el("button", { class: "link-btn", type: "button", onclick: () => invoke("open_link", { which: "opensubtitles" }).catch((e) => toast(String(e), true)) }, "No account yet? Make a free one");
  return el("div", { class: "subs-form" },
    el("label", { class: "field" }, el("small", { text: "OpenSubtitles name" }), user),
    el("label", { class: "field" }, el("small", { text: "Password" }), pass),
    el("div", { class: "subs-acts" }, go, signUp), err);
}

/** The last setup screen: the same sign-in, as a choice. */
export async function subtitleOffer(name) {
  const st = await invoke("subs_status").catch(() => null);
  if (!st || st.ready) return null;
  const box = el("div", { class: "done-note" });
  const yes = el("button", { class: "btn small primary", type: "button", onclick: () => {
    box.replaceChildren(el("strong", { text: "More subtitles" }), subtitleForm(name, st.installed, () => box.remove()));
  } }, "Set it up");
  const no = el("button", { class: "btn small ghost", type: "button", onclick: () => box.remove() }, "Skip");
  box.append(el("strong", { text: "Want more subtitles?" }),
    el("p", { class: "note", text: `${SUBS_PITCH} You can also do this later in Settings.` }),
    el("div", { class: "subs-acts" }, yes, no));
  return box;
}

async function subtitleBlock(name) {
  const st = await invoke("subs_status").catch(() => ({ installed: false, ready: false }));
  const box = el("div", {}, el("h2", { text: "More subtitles", style: "margin-top:36px;font-size:1.2rem" }));
  if (st.ready) {
    box.append(el("div", { class: "box" }, el("div", { class: "opt" },
      el("span", {}, el("b", { text: `On, as ${st.user}` }), el("small", { text: "While watching, open the tracks menu and pick Download subtitles." })))));
    return box;
  }
  const open = el("button", { class: "btn small ghost", onclick: () => {
    box.replaceChildren(box.firstChild, subtitleForm(name, st.installed, () => renderSettings($("#view"), "account")));
  } }, "Set it up");
  box.append(el("div", { class: "box" }, el("div", { class: "opt" }, el("span", {}, el("small", { text: SUBS_PITCH })), open)));
  return box;
}

/* ---------- Apps (paths and ports) ---------- */
function portOf(url) { const m = /:(\d+)/.exec((url || "").split("://")[1] || ""); return m ? m[1] : ""; }

async function apps(panel) {
  const configs = await invoke("get_configs");
  const settings = store.settings;
  const override = (id) => { let o = settings.apps.find((a) => a.id === id); if (!o) { o = { id, exe: null, port: null, managed: true }; settings.apps.push(o); } return o; };
  const save = async () => { store.settings = settings; return invoke("set_settings", { s: settings }).catch((e) => { toast(String(e), true); return null; }); };
  // Chillarr only keeps a path that is the app's own program (Sonarr.exe for Sonarr).
  const setExe = async (id, input, path) => {
    override(id).exe = path;
    const cfgs = await save();
    if (path && cfgs && !cfgs.some((c) => c.id === id && c.exe === path)) {
      override(id).exe = null;
      input.value = cfgs.find((c) => c.id === id)?.exe || "";
      toast(`That isn't ${LABEL[id]}'s program.`, true);
    }
  };
  panel.append(el("h2", { text: "Apps" }), el("p", { class: "sub", text: "Chillarr finds all of this itself. Change it only if something is wrong." }),
    // The manga downloader is part of Chillarr: its program and port are Chillarr's own.
    el("div", {}, ORDER.filter((id) => id !== "Suwayomi").map((id) => {
      const c = configs.find((x) => x.id === id) || {};
      const exe = el("input", { type: "text", value: c.exe || "", placeholder: c.service ? `Windows service "${c.service}"` : "Not found", spellcheck: "false" });
      const port = el("input", { type: "number", min: "1", max: "65535", value: portOf(c.base_url) });
      const managed = el("input", { type: "checkbox", class: "switch", "aria-label": `Let Chillarr start ${LABEL[id]}` });
      managed.checked = c.managed !== false;
      exe.addEventListener("change", () => setExe(id, exe, exe.value.trim() || null));
      port.addEventListener("change", () => { const n = parseInt(port.value, 10); override(id).port = n >= 1 && n <= 65535 ? n : null; save(); });
      managed.addEventListener("input", () => { override(id).managed = managed.checked; save(); });
      const browse = el("button", { class: "btn small ghost", onclick: async () => { const p = await invoke("pick_exe"); if (p) { exe.value = p; setExe(id, exe, p); } } }, "Browse");
      return el("div", { class: "app-row" }, el("div", { class: "name", text: LABEL[id] }),
        el("label", { class: "field" }, el("small", { text: "Program" }), el("span", { class: "pick" }, exe, browse)),
        el("label", { class: "field" }, el("small", { text: "Port" }), port),
        el("label", { class: "field" }, el("small", { text: "Manage" }), managed));
    })),
    el("div", { style: "margin-top:18px" }, el("button", { class: "btn ghost", onclick: async () => { await invoke("rediscover"); renderSettings($("#view"), "apps"); toast("Looked for every app again."); } }, "Find again")));
}

/* ---------- General ---------- */
function general(panel) {
  const s = store.settings;
  const sw = (key, title, hint) => {
    const box = el("input", { type: "checkbox", class: "switch" });
    box.checked = !!s[key];
    box.addEventListener("input", async () => { s[key] = box.checked; await invoke("set_settings", { s }); });
    return el("label", { class: "opt" }, el("span", {}, el("b", { text: title }), el("small", { text: hint })), box);
  };
  const vlc = !!store.media?.vlc;
  const seg = el("div", { class: "segmented", role: "group", "aria-label": "Play with" });
  const drawSeg = () => seg.replaceChildren(
    el("button", { class: s.player !== "vlc" ? "on" : "", onclick: async () => { s.player = "chillarr"; drawSeg(); await invoke("set_settings", { s }); } }, "Chillarr"),
    el("button", { class: s.player === "vlc" ? "on" : "", disabled: !vlc, title: vlc ? "" : "VLC isn't installed", onclick: async () => { s.player = "vlc"; drawSeg(); await invoke("set_settings", { s }); } }, "VLC"));
  drawSeg();
  const voices = audioChoice(() => s, (change) => { Object.assign(s, change); invoke("set_settings", { s }).catch((e) => toast(String(e), true)); });
  panel.append(el("h2", { text: "General" }),
    el("div", { class: "box" },
      el("div", { class: "opt" }, el("span", {}, el("b", { text: "Original voices or dubbed?" }), el("small", { text: "For new downloads. A title with no dub still downloads, in its original voices. The player's audio button switches when a file has both." })), voices),
      subSizeRow(),
      sw("start_with_windows", "Start with Windows", "Opens in the tray when you sign in."),
      seedSwitch()),
    ...(WEB ? [] : [limitBox(), gpuBox(), sharpenBox()]),
    remoteBox(),
    more("More options",
      el("div", { class: "box" },
        el("div", { class: "opt" }, el("span", {}, el("b", { text: "Play with" }), el("small", { text: vlc ? "VLC plays the original file at full quality." : "Install VLC to play original files at full quality." })), seg),
        sw("auto_fix", "Fix automatically", "Checks every minute and fixes what breaks."),
        sw("defender_ask", "Defender access", "If Windows Defender blocks your apps, Fix everything asks before letting them through."),
        sw("skip_dubs", "Skip extra dubs", "Avoids copies that could start in the wrong language, like a Hindi or Italian dub."),
        sw("skip_remux", "Skip huge copies", "Avoids copies of 25 GB or more, so a 1080p movie stays under about 8 GB.")),
      el("div", { style: "margin-top:24px" }, el("button", { class: "btn ghost", onclick: () => openSetup() }, "Run the setup again"))));
}

/** The graphics card for converting video. Only ever in this PC's window, like Limit downloads. */
function gpuBox() {
  const sw = el("input", { type: "checkbox", class: "switch", disabled: true });
  const hint = el("small", { text: "Checking your graphics card..." });
  const draw = (st) => {
    sw.checked = st.on;
    sw.disabled = !st.card && !st.on;
    hint.textContent = st.card
      ? `Converts video on your ${st.card} instead of the processor. Can help stutter on a TV, and leaves more for your games. Applies to the next video you start.`
      : "No graphics card here can convert video, so the processor does it.";
  };
  invoke("gpu_status").then(draw).catch((e) => { hint.textContent = String(e); });
  sw.addEventListener("input", async () => {
    sw.disabled = true;
    try { draw(await invoke("gpu_set", { on: sw.checked })); }
    catch (e) { toast(String(e), true); sw.checked = !sw.checked; sw.disabled = false; }
  });
  return el("div", { class: "box", style: "margin-top:24px" }, el("label", { class: "opt" }, el("span", {}, el("b", { text: "Use graphics card for video" }), hint), sw));
}

/** Sharpen manga pages with the graphics card (sharpen.rs). This PC's window only. */
function sharpenBox() {
  const sw = el("input", { type: "checkbox", class: "switch" });
  const hint = el("small", { text: "Redraws each chapter you download at twice the detail, once, in the background. Pauses while a game is full screen." });
  const state = el("small", { class: "sharpen-state" });
  const draw = (st) => {
    sw.checked = st.on;
    if (!st.on) state.textContent = "";
    else if (st.paused) state.textContent = "Paused while something is full screen.";
    else if (st.now) state.textContent = `Sharpening ${st.now}${st.left > 1 ? `, ${st.left - 1} more after it` : ""}. Copies use ${st.mb} MB.`;
    else state.textContent = `All caught up. Copies use ${st.mb} MB. Turning this off deletes them; your chapters stay.`;
  };
  const poll = () => invoke("sharpen_status").then(draw).catch(() => {});
  poll();
  const t = setInterval(() => { if (!sw.isConnected) clearInterval(t); else if (!sw.disabled) poll(); }, 4000);
  sw.addEventListener("input", async () => {
    sw.disabled = true;
    if (sw.checked) state.textContent = "Getting it ready (a 45 MB download the first time)...";
    try { draw(await invoke("sharpen_set", { on: sw.checked })); }
    catch (e) { toast(String(e), true); sw.checked = !sw.checked; state.textContent = ""; }
    sw.disabled = false;
  });
  return el("div", { class: "box", style: "margin-top:24px" },
    el("label", { class: "opt" }, el("span", {}, el("b", { text: "Sharpen pages with your graphics card" }), hint, state), sw));
}

/** Limit downloads. Only ever in this PC's window: the web pages don't draw it and can't reach its commands. */
function limitBox() {
  const box = el("div", { class: "box", style: "margin-top:24px" });
  let st = null;
  const push = async (change) => {
    try { st = await invoke("limit_set", { on: st.on, hostFree: st.host_free, downMbps: st.down_mbps, upMbps: st.up_mbps, ...change }); }
    catch (e) { toast(String(e), true); st = await invoke("limit_status"); }
    draw();
  };
  const draw = () => {
    const on = el("input", { type: "checkbox", class: "switch" });
    on.checked = st.on;
    on.addEventListener("input", async () => {
      // First time on: nothing to work the share out of yet.
      if (on.checked && !(st.down_mbps > 0 && st.up_mbps > 0)) { await measure(true); return; }
      push({ on: on.checked });
    });
    const rows = [el("label", { class: "opt" }, el("span", {}, el("b", { text: "Limit downloads" }),
      el("small", { text: `All downloads together use at most ${st.percent}% of your internet, so games on this PC stay smooth. Only you can see this, here on this PC.` })), on)];
    if (st.on) {
      const free = el("input", { type: "checkbox", class: "switch" });
      free.checked = st.host_free;
      free.addEventListener("input", () => push({ hostFree: free.checked }));
      rows.push(el("label", { class: "opt" }, el("span", {}, el("b", { text: "Don't limit what I start here" }),
        el("small", { text: "Downloads you start in this window run at full speed. Ones started on a phone, TV or browser stay limited." })), free));
    }
    const num = (value, label, key) => {
      const i = el("input", { type: "number", min: "1", step: "1", value: value > 0 ? String(value) : "", "aria-label": label, class: "limit-num" });
      i.addEventListener("change", () => { const n = Number(i.value); if (n > 0) push({ [key]: n }); else { i.value = value > 0 ? String(value) : ""; toast("Type a speed above 0.", true); } });
      return i;
    };
    const go = el("button", { class: "btn small ghost", onclick: () => measure(false) }, "Measure");
    measureBtn = go;
    rows.push(el("div", { class: "opt" }, el("span", {}, el("b", { text: "Your internet speed" }),
      el("small", { text: st.down_mbps > 0 ? `Downloads can use up to ${st.cap_down_mbps} Mbps down and ${st.cap_up_mbps} Mbps up. Measuring uses your full speed for about 15 seconds.` : "Measuring uses your full speed for about 15 seconds." })),
      el("div", { class: "limit-speed" }, el("label", {}, el("small", { text: "Down" }), num(st.down_mbps, "Download speed in Mbps", "downMbps")), el("label", {}, el("small", { text: "Up" }), num(st.up_mbps, "Upload speed in Mbps", "upMbps")), el("small", { text: "Mbps" }), go)));
    box.replaceChildren(...rows);
  };
  let measureBtn = null;
  async function measure(turnOn) {
    if (measureBtn) { measureBtn.disabled = true; measureBtn.textContent = "Measuring..."; }
    try {
      const got = await invoke("limit_measure");
      await push({ downMbps: got.down_mbps, upMbps: got.up_mbps, ...(turnOn ? { on: true } : {}) });
    } catch (e) { toast(String(e), true); draw(); }
  }
  invoke("limit_status").then((s) => { st = s; draw(); }).catch((e) => box.replaceChildren(el("p", { class: "sub", text: String(e) })));
  return box;
}

/** Subtitle size, with a sample that changes as the slider moves. */
function subSizeRow() {
  const pct = (x) => `${Math.round(x * 100)}%`;
  const range = el("input", { type: "range", min: "0.6", max: "2", step: "0.05", value: String(subScale()), "aria-label": "Subtitle size" });
  const label = el("span", { class: "sub-size-val", text: pct(subScale()) });
  const sample = el("div", { class: "sub-sample" }, el("span", { text: "This is how big subtitles are." }));
  const apply = () => { setSubScale(Number(range.value)); label.textContent = pct(subScale()); sample.style.fontSize = `${22 * subScale()}px`; };
  range.addEventListener("input", apply);
  sample.style.fontSize = `${22 * subScale()}px`;
  return el("div", { class: "opt sub-size" }, el("span", {}, el("b", { text: "Subtitle size" }),
    el("small", { text: "100% is the size the subtitles were made at." }), sample), el("div", { class: "sub-size-ctl" }, range, label));
}

/** Seeding: qBittorrent's own setting, read from it and written back to it. */
function seedSwitch() {
  const box = el("input", { type: "checkbox", class: "switch", disabled: true });
  const hint = el("small", { text: "Keep sharing finished downloads with others. Off clears each one as soon as it's in your library." });
  invoke("seeding").then((on) => { box.checked = on; box.disabled = false; }).catch(() => { hint.textContent = "Needs qBittorrent running. Press Fix everything first."; });
  box.addEventListener("input", async () => {
    box.disabled = true;
    try { toast(await invoke("set_seeding", { on: box.checked })); } catch (e) { toast(String(e), true); box.checked = !box.checked; }
    box.disabled = false;
  });
  return el("label", { class: "opt" }, el("span", {}, el("b", { text: "Seed after downloading" }), hint), box);
}

/** Phone remote and controllers: how to drive Chillarr from the couch. */
function remoteBox() {
  const box = el("div", { class: "box", style: "margin-top:24px" });
  const draw = (st) => {
    const sw = el("input", { type: "checkbox", class: "switch" });
    sw.checked = st.on;
    sw.addEventListener("input", async () => draw(await invoke("remote_set", { on: sw.checked, newCode: false })));
    const rows = [el("label", { class: "opt" }, el("span", {}, el("b", { text: "Phone remote" }),
      el("small", { text: "Use your phone as a remote, and type on it to search. A TV shows its own code under Settings > Phone remote." })), sw)];
    if (st.on) {
      rows.push(el("div", { class: "opt remote-info" },
        el("span", {}, el("small", { text: "On your phone, open" }), remoteUrls(st.urls),
          el("small", { text: "and type this code once to control Chillarr on this PC:" }), el("p", { class: "addr code", text: spaced(st.pin) }),
          el("small", { text: "If Windows asks whether Chillarr may use the network, allow private networks." })),
        el("button", { class: "btn small ghost", onclick: async () => draw(await invoke("remote_set", { on: true, newCode: true })) }, "New code")));
      for (const d of st.devices || []) {
        rows.push(el("div", { class: "opt" }, el("span", {}, el("b", { text: d.name }), el("small", { text: `Its code: ${spaced(d.code)}` })),
          el("button", { class: "btn small ghost", onclick: async () => draw(await invoke("remote_forget", { id: d.id })) }, "Forget")));
      }
    }
    rows.push(el("div", { class: "opt" }, el("span", {}, el("b", { text: "Game controllers" }),
      el("small", { text: "Xbox and PlayStation controllers just work: D-pad or stick to move, A to choose, B to go back." }))));
    box.replaceChildren(...rows);
  };
  invoke("remote_status").then(draw).catch(() => {});
  return box;
}

const spaced = (code) => String(code || "").replace(/^(.{5})(.{5})$/, "$1 $2");

/** Each address a phone can open: home Wi-Fi first, then Tailscale. */
function remoteUrls(urls) {
  if (!urls || !urls.length) return el("p", { class: "addr", text: "Starting..." });
  return el("div", {}, urls.map((u, i) => el("p", { class: "addr", text: i ? `${u}  (Tailscale)` : u })));
}

/** A name for this device, shown on the phone and on the PC. */
function deviceName() {
  const ua = navigator.userAgent;
  if (/Web0S|webOS|NetCast/i.test(ua)) return "LG TV";
  if (/Tizen/i.test(ua)) return "Samsung TV";
  if (/Android TV|AFT|BRAVIA|SMART-TV|SmartTV/i.test(ua)) return "TV";
  if (/iPhone/.test(ua)) return "iPhone";
  if (/iPad/.test(ua)) return "iPad";
  if (/Android/.test(ua)) return "Android phone";
  return "Browser";
}

/** In Jellyfin's pages: this device's own phone-remote code. */
function webRemote(panel) {
  const box = el("div", { class: "box" });
  // The key is what makes this device this device: kept in the browser by the shim.
  const keep = window.__CHILLARR_REMOTE__;
  const myCode = async (renew) => {
    const st = await invoke("remote_device", { name: deviceName(), key: keep ? keep.key() || null : null, renew });
    if (keep && st.on) keep.setKey(st.key);
    return st;
  };
  const draw = (st) => {
    if (!st.on) {
      box.replaceChildren(el("div", { class: "opt" }, el("span", {}, el("b", { text: "The phone remote is off" }),
        el("small", { text: "Turn it on in Chillarr on the PC, under Settings > General > Phone remote. Then come back here for this device's code." }))));
      return;
    }
    const renew = el("button", { class: "btn small ghost", onclick: async () => {
      renew.disabled = true;
      try { draw(await myCode(true)); toast("New code. Phones with the old one have to type this one."); }
      catch (e) { toast(String(e), true); renew.disabled = false; }
    } }, "New code");
    box.replaceChildren(el("div", { class: "opt remote-info" },
      el("span", {}, el("small", { text: "On your phone, open" }), remoteUrls(st.urls),
        el("small", { text: "and type this code once:" }), el("p", { class: "addr code", text: spaced(st.code) }),
        el("small", { text: "The code belongs to this device and stays the same until you ask for a new one. Works on the same Wi-Fi, or anywhere over Tailscale." })),
      renew));
  };
  panel.append(el("h2", { text: "Phone remote" }), el("p", { class: "sub", text: "Control this screen from your phone: arrows, OK, back, play and pause, and typing to search." }), box);
  box.append(el("p", { class: "sub", text: "Getting this device's code..." }));
  myCode(false).then(draw).catch((e) => box.replaceChildren(el("p", { class: "sub", text: String(e) })));
}

/* ---------- In Jellyfin's web pages ---------- */
/** Who's signed in, and the way back to jellyfin-web's own pages. */
function webAccount(panel) {
  const jf = (hash) => el("a", { class: "btn small ghost", href: `../index.html#/${hash}` }, "Open");
  const row = (title, hint, act) => el("div", { class: "opt" }, el("span", {}, el("b", { text: title }), el("small", { text: hint })), act);
  const out = el("button", { class: "btn small ghost", onclick: () => { out.disabled = true; window.__CHILLARR_JF__?.signOut(); } }, "Sign out");
  panel.append(el("h2", { text: "Account" }),
    el("p", { class: "sub", text: `Signed in to Jellyfin as ${viewer.name}${viewer.admin ? ", an administrator" : ""}. What you watch and your My List follow this account to Jellyfin's TV and phone apps.` }),
    el("div", { class: "box" },
      row("Your Jellyfin settings", "Your password, picture, languages, and how the TV and phone apps look.", jf("mypreferencesmenu")),
      viewer.admin ? row("Jellyfin dashboard", "Users, libraries and the server itself.", jf("dashboard")) : null,
      row("Sign out", "Goes back to Jellyfin's sign-in, where someone else can sign in.", out)),
    // Hidden things: on phones too, or what's hidden here could never be seen again.
    hiddenBox());
}

/** Subtitle size and controllers; seeding for an admin. Kept in this browser or on the server. */
function webGeneral(panel) {
  panel.append(el("h2", { text: "Playback" }), el("p", { class: "sub", text: "How things play in this browser." }),
    el("div", { class: "box" },
      subSizeRow(),
      viewer.admin ? seedSwitch() : null,
      el("div", { class: "opt" }, el("span", {}, el("b", { text: "Game controllers" }),
        el("small", { text: "Xbox, PlayStation and other controllers work here too: D-pad or stick to move, A to choose, B to go back, LB and RB to switch tabs." })))));
}

/* ---------- Logs ---------- */
/** Saves one file with everything needed to find a problem, and shows it. */
export function saveLogsButton(cls = "btn primary") {
  const b = el("button", { class: cls, type: "button" }, "Save logs");
  b.addEventListener("click", async () => {
    b.disabled = true;
    b.textContent = "Saving...";
    try {
      const path = await invoke("logs_save");
      toast(`Saved ${path}. Send that file. Passwords and keys are hidden.`);
      invoke("open_folder", { path: path.replace(/[\\/][^\\/]*$/, "") }).catch(() => {});
    } catch (e) { toast(String(e), true); }
    b.disabled = false;
    b.textContent = "Save logs";
  });
  return b;
}

async function logs(panel) {
  const list = el("ol", { class: "log" });
  const item = (l) => el("li", { dataset: { level: l.level } }, el("time", { text: l.at }), el("span", { text: l.text }));
  const seq = settingsSeq;
  const all = await invoke("get_logs").catch(() => []);
  // Another section (or page) opened while they loaded.
  if (seq !== settingsSeq) return;
  list.replaceChildren(...all.map(item));
  panel.append(el("div", { class: "panel-head" }, el("div", {}, el("h2", { text: "Logs" }), el("p", { class: "sub", text: "What Chillarr did, newest first. API keys are hidden." })), saveLogsButton()), list);
  unsub = store.on((w) => {
    if (w !== "logs") return;
    list.prepend(item(store.logs[0]));
    if (list.children.length > 500) list.lastChild.remove();
  });
}
