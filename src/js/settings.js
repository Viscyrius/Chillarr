// Settings: the six apps and Fix everything, indexers, library folders,
// app paths, the Jellyfin account and streaming, general options, logs.
import { $, ask, el, icon, ICON, invoke, LABEL, ORDER, store, toast, viewer, WEB } from "./api.js";
import { openSetup } from "./setup.js";
import { setSubScale, subScale } from "./subrender.js";

const SECTIONS = [
  ["system", "System"],
  ["indexers", "Indexers"],
  ["library", "Library & storage"],
  ["account", "Jellyfin & streaming"],
  ["apps", "Apps"],
  ["general", "General"],
  ["logs", "Logs"],
];
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

export function renderSettings(view, section = "system") {
  // Library and Storage were two pages; old links open the merged one.
  if (section === "storage") section = "library";
  const sections = WEB ? WEB_SECTIONS() : SECTIONS;
  if (!sections.some(([id]) => id === section)) section = sections[0][0];
  if (unsub) unsub();
  unsub = null;
  const nav = el("nav", { class: "settings-nav", "aria-label": "Settings" }, el("h1", { text: "Settings" }),
    sections.map(([id, name]) => el("button", { class: id === section ? "on" : "", onclick: () => (location.hash = `settings/${id}`) }, name)));
  const panel = el("section", { class: "panel" });
  view.replaceChildren(el("div", { class: "settings" }, nav, panel));
  (WEB ? { account: webAccount, general: webGeneral, remote: webRemote, library } : { system, indexers, library, account, apps, general, logs })[section](panel);
}

/* ---------- System ---------- */
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

function system(panel) {
  const fix = el("button", { class: "btn primary", onclick: () => invoke("fix_everything") }, "Fix everything");
  const summary = el("p", { class: "sub" });
  const cardsBox = el("div", { class: "apps" });
  const chainBox = el("div");
  const draw = () => {
    const list = ORDER.map((id) => store.cards.get(id)).filter(Boolean);
    const bad = list.filter((c) => c.level === "red" || c.level === "amber");
    summary.textContent = store.fixing ? "Starting what's down, then testing every indexer." : bad.length ? `${bad.length} of ${list.length} need attention. Fix everything starts what's down, re-tests every indexer and connects the apps.` : "Every app is running and every indexer works.";
    fix.disabled = store.fixing;
    fix.textContent = store.fixing ? "Fixing..." : "Fix everything";
    cardsBox.replaceChildren(...list.map((c, j) => { const n = appCard(c); n.style.setProperty("--j", j); if (cardsBox.dataset.drawn) n.style.animation = "none"; return n; }));
    cardsBox.dataset.drawn = "1";
    chainBox.replaceChildren(chain());
  };
  panel.append(el("div", { class: "panel-head" }, el("div", {}, el("h2", { text: "System" }), summary), fix), chainBox, cardsBox);
  draw();
  unsub = store.on((w) => { if (w === "cards" || w === "fixing") draw(); });
}

async function offerInstall(ids) {
  const one = ids.length === 1;
  const ok = await ask(one ? `Install ${LABEL[ids[0]]}?` : `Install ${ids.length} missing apps?`,
    one ? `Chillarr downloads ${WHERE[ids[0]]}. Windows may ask for permission.` : "Chillarr downloads each one from its official source. Windows may ask for permission.",
    one ? [] : ids.map((id) => LABEL[id]), "Install");
  if (!ok) return false;
  if (one) {
    try { await invoke("install_app", { id: ids[0] }); } catch (e) { toast(String(e), true); }
    return true;
  }
  // All at once, then each is started and everything is connected.
  toast(`Installing ${ids.length} apps at once. Chillarr sets them up and connects them when they're done.`);
  const failed = await invoke("install_apps", { ids }).catch((e) => [[null, String(e)]]);
  for (const [id, e] of failed) toast(id ? `${LABEL[id]}: ${e}` : e, true);
  if (!failed.length) toast("All installed and connected.");
  return true;
}
export { offerInstall };

async function offerUpdate(c) {
  const ok = await ask(`Update ${LABEL[c.id]} to ${c.update}?`, `${LABEL[c.id]} closes for about a minute while it updates, then opens again. Downloads pause and pick up where they left off.`, [], "Update");
  if (ok) { try { await invoke("update_app", { id: c.id }); } catch (e) { toast(String(e), true); } }
}

/* ---------- Indexers ---------- */
// Each one passed Prowlarr's test and returned results on 2026-10-18.
// Anidex (down), Uindex and TorrentsCSV (rate-limited Sonarr and Radarr's sync) were left out.
const RECOMMENDED = [
  { def: "1337x", name: "1337x", why: "Huge catalogue of movies and series. Goes through FlareSolverr." },
  { def: "Knaben", name: "Knaben", why: "Searches many big sites at once. Great for movies and series." },
  { def: "thepiratebay", name: "The Pirate Bay", why: "The biggest general site." },
  { def: "limetorrents", name: "LimeTorrents", why: "Movies and series, a good backup." },
  { def: "yts", name: "YTS", why: "Small, good-quality movie files." },
  { def: "eztv", name: "EZTV", why: "TV episodes, fast after they air." },
  { def: "nyaasi", name: "Nyaa.si", why: "The go-to site for anime." },
  { def: "animetosho-xyz", name: "Anime Tosho", why: "Anime, mirrors Nyaa and more." },
  { def: "SubsPlease", name: "SubsPlease", why: "New anime episodes, out within hours." },
  { def: "nekobt", name: "nekoBT", why: "Anime, well sorted releases." },
  { def: "tokyotosho", name: "Tokyo Toshokan", why: "Older and rarer anime." },
];

export async function suggestions(onAdded) {
  const info = await invoke("setup_indexers");
  const have = new Set(info.have.map((i) => i.definition));
  const adds = [];
  const cards = RECOMMENDED.map((r) => {
    const btn = el("button", { class: have.has(r.def) ? "btn small ghost" : "btn small primary", disabled: have.has(r.def) }, have.has(r.def) ? "Added" : "Add");
    // Quiet when Add all runs it: one toast at the end instead of eleven.
    const add = async (quiet = false) => {
      if (btn.disabled) return true;
      btn.disabled = true; btn.textContent = "Adding...";
      try { const msg = await invoke("setup_add_indexer", { definition: r.def }); if (!quiet) toast(msg); btn.textContent = "Added"; btn.className = "btn small ghost"; onAdded?.(); return true; }
      catch (e) { if (!quiet) toast(String(e), true); btn.disabled = false; btn.textContent = "Add"; return `${r.name}: ${e}`; }
    };
    adds.push(add);
    btn.addEventListener("click", () => add());
    return el("div", { class: "suggest-card" }, el("div", {}, el("strong", { text: r.name }), el("span", { text: r.why })), btn);
  });
  const missing = RECOMMENDED.filter((r) => !have.has(r.def)).length;
  const all = el("button", { class: "btn primary", disabled: !missing }, missing ? "Add all" : "All added");
  all.addEventListener("click", async () => {
    all.disabled = true; all.textContent = "Adding...";
    // One at a time: Prowlarr tests each site as it's added.
    const failed = [];
    for (const add of adds) { const r = await add(true); if (r !== true) failed.push(r); }
    all.textContent = failed.length ? "Add the rest" : "All added";
    all.disabled = !failed.length;
    toast(failed.length ? `Added the others. Couldn't add ${failed.join("; ")}` : "Added every recommended site.", failed.length > 0);
  });
  return el("div", {}, el("div", { class: "suggest-all" }, all), el("div", { class: "suggest" }, cards));
}

async function indexers(panel) {
  const table = el("tbody");
  const test = el("button", { class: "btn ghost", onclick: async () => { test.disabled = true; test.textContent = "Testing..."; draw(await invoke("test_indexers")); test.disabled = false; test.textContent = "Test all"; } }, "Test all");
  const cell = (c) => {
    if (!c) return el("td", { class: "none", text: "Not used" });
    const td = el("td", { title: c.reason });
    td.append(el("span", { class: "dot", dataset: { level: c.level } }), c.level === "ok" ? "Working" : c.reason);
    return td;
  };
  const draw = (rows) => table.replaceChildren(...rows.map((r) => el("tr", {}, el("td", { text: r.name }), cell(r.prowlarr), cell(r.sonarr), cell(r.radarr))));
  panel.append(
    el("div", { class: "panel-head" }, el("div", {}, el("h2", { text: "Indexers" }), el("p", { class: "sub", text: "The sites Prowlarr searches, and whether Sonarr and Radarr can use them." })), test),
    el("table", { class: "grid" }, el("thead", {}, el("tr", {}, ["Indexer", "Prowlarr", "Sonarr", "Radarr"].map((h) => el("th", { text: h })))), table),
    el("h2", { text: "Recommended", style: "margin-top:36px;font-size:1.2rem" }),
    await suggestions(async () => draw(await invoke("indexer_table")))
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
    el("p", { class: "sub", text: "Where your downloads go, how full your drives are, and clearing out what you've already watched. Each folder is also a library in Jellyfin." }));
  const drivesBox = el("div", {}, el("p", { class: "note", text: "Measuring..." }));
  const foldersBox = el("div");
  const roomBox = el("div");
  const settingsBox = el("div");
  panel.append(drivesBox, foldersBox, roomBox, settingsBox);
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
function storageParts(o) {
  let filter = "all";
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
    const ok = await ask(`Delete ${watched.length} watched title${watched.length === 1 ? "" : "s"}?`, `This frees ${size(watchedSize)}. The files are deleted from your PC; you can always download them again.`, watched.map((i) => `${i.tile.title} (${size(i.size)})`), "Delete");
    if (!ok) return;
    sweep.disabled = true;
    for (const [n, i] of watched.entries()) {
      sweep.textContent = `Deleting ${n + 1} of ${watched.length}...`;
      try { await invoke("media_delete", { tile: i.tile }); } catch (e) { toast(String(e), true); }
    }
    toast(`Freed about ${size(watchedSize)}.`);
    renderSettings($("#view"), "library");
  });
  const tidy = el("div", { class: "box" },
    el("div", { class: "opt" }, el("span", {}, el("b", { text: "Clean up watched" }),
      el("small", { text: "Works per episode. Each episode you watch is deleted this many days later, and won't download again. Movies go once you finish them. Episodes you haven't watched and new ones stay. Nothing is deleted while another profile is still watching it. Off unless you pick a time." })), cleanup),
    el("div", { class: "opt" }, el("span", {}, el("b", { text: `${o.items.length} titles, ${size(total)}` }),
      el("small", { text: `${watched.length} watched (${size(watchedSize)}).` })), sweep));

  const chips = el("div", { class: "st-chips" });
  const list = el("ul", { class: "st-list" });
  const biggest = o.items[0]?.size || 1;
  const draw = () => {
    chips.replaceChildren(...[["all", "All"], ["watched", "Watched"], ["unwatched", "Not watched"]].map(([id, label]) =>
      el("button", { class: id === filter ? "chip on" : "chip", onclick: () => { filter = id; draw(); } }, label)));
    const shown = o.items.filter((i) => filter === "all" || (filter === "watched") === i.watched);
    list.replaceChildren(...shown.map((i) => {
      const del = el("button", { class: "btn small ghost danger" }, "Delete");
      del.addEventListener("click", async () => {
        const ok = await ask(`Delete ${i.tile.title}?`, `This deletes it from your PC and frees ${size(i.size)}. You can always download it again.`, [], "Delete");
        if (!ok) return;
        del.disabled = true;
        try { toast(await invoke("media_delete", { tile: i.tile })); renderSettings($("#view"), "library"); } catch (e) { toast(String(e), true); del.disabled = false; }
      });
      const kind = i.tile.kind === "series" ? "Series" : "Movie";
      return el("li", {},
        el("div", { class: "st-thumb", style: i.tile.poster ? `background-image:url("${i.tile.poster.replace(/"/g, "%22")}")` : "" }),
        el("div", { class: "st-info" },
          el("b", { text: i.tile.title }),
          el("small", { text: [kind, i.tile.year, i.watched ? "Watched" : null].filter(Boolean).join(" · ") }),
          el("div", { class: "st-bar thin" }, el("span", { style: `width:${Math.max(2, Math.round((i.size / biggest) * 100))}%` }))),
        el("span", { class: "st-size", text: size(i.size) }), del);
    }));
    if (!shown.length) list.append(el("li", { class: "st-empty" }, filter === "watched" ? "Nothing watched yet." : "Nothing here."));
  };
  draw();
  return [tidy, subhead("What takes the most room"), chips, list];
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
  return [
    subhead("Change folders", "margin-top:36px"),
    downloadsRow(),
    el("p", { class: "note", text: "A folder with \"Anime\" in its name is used for anime. Removing a folder here doesn't delete anything." }),
    el("div", { class: "folder-step", style: "margin-top:16px" },
      folderPicker("Movies", "Radarr puts movies here.", movies),
      folderPicker("Series and anime", "Sonarr puts series here. Add a separate folder named Anime for anime.", series)),
    err, el("div", { style: "margin-top:18px" }, save)];
}

/* ---------- Jellyfin & streaming ---------- */
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
  panel.append(el("h2", { text: "Jellyfin & streaming" }),
    el("p", { class: "sub", text: "Jellyfin plays your library in Chillarr and streams it to your phone, TV or laptop. Nothing goes through the cloud: your PC is the server." }));
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
      el("div", { class: "opt" }, el("span", {}, el("b", { text: `Signed in as ${ms.user}` }), el("small", { text: "Chillarr uses this account to play and track what you watch." })),
        el("button", { class: "btn small ghost", onclick: async () => { await invoke("media_sign_out"); renderSettings($("#view"), "account"); } }, "Sign out"))));
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
      }
      catch (e) { err.textContent = String(e); go.disabled = false; }
    });
    panel.append(el("p", { class: "note", text: ms.setup_done ? "Sign in with your Jellyfin account. qBittorrent, Sonarr, Radarr and Prowlarr get the same login." : "Jellyfin is new. Pick a name and password; Chillarr sets it up with your library folders, and qBittorrent, Sonarr, Radarr and Prowlarr get the same login." }),
      el("div", { style: "display:grid;gap:14px;max-width:420px;margin-top:14px" },
        el("label", { class: "field" }, el("small", { text: "Name" }), user), el("label", { class: "field" }, el("small", { text: "Password" }), pass), el("div", {}, go), err));
  }
  if (ms.signed_in) {
    panel.append(el("div", { class: "box", style: "margin-top:24px" },
      el("div", { class: "opt" }, el("span", {}, el("b", { text: "Profiles" }),
        el("small", { text: "Everyone gets their own Continue watching and My List. Each profile is a Jellyfin account too, so it carries over to Jellyfin's TV and phone apps. Add a PIN to keep one private." })),
        el("button", { class: "btn small ghost", onclick: () => window.dispatchEvent(new Event("chillarr:profiles")) }, "Manage profiles")),
      el("div", { class: "opt" }, el("span", {}, el("b", { text: "Browsers that know the main profile" }),
        el("small", { text: "Browsers that know the main profile's password will ask for it again (a lost phone, a lent laptop)." })),
        el("button", { class: "btn small ghost", onclick: async (e) => {
          const b = e.currentTarget;
          b.disabled = true;
          try { await invoke("profiles_forget_web_devices"); toast("Every browser asks for the main profile's password again."); }
          catch (err) { toast(String(err), true); }
          b.disabled = false;
        } }, "Forget browsers"))));
    panel.append(appsLoginBlock(ms.user), shelfBlock(), searchBlock(), await subtitleBlock(ms.user));
  }
  panel.append(el("h2", { text: "Watch on other devices", style: "margin-top:36px;font-size:1.2rem" }),
    el("p", { class: "note", text: "Install the free Jellyfin app on your phone or TV and connect to this address while on the same Wi-Fi. Jellyfin runs while Chillarr is running, and Chillarr starts with Windows." }),
    el("p", { class: "addr", text: ms.address || "http://this-pc:8096" }),
    el("h2", { text: "Watch away from home", style: "margin-top:30px;font-size:1.2rem" }),
    ms.tailscale || ms.tailscale_ip
      ? el("div", {},
        el("p", { class: "note", text: "Tailscale is connected on this PC. Install Tailscale on your phone or laptop, sign in with the same account, then open the Jellyfin app and connect to:" }),
        el("p", { class: "addr selectable", text: ms.tailscale || ms.tailscale_ip }),
        ms.tailscale && ms.tailscale_ip ? el("p", { class: "note", text: `If that name doesn't work in an app, use ${ms.tailscale_ip}` }) : null,
        el("p", { class: "note", text: "It works anywhere with internet, stays private (only your own devices can reach it), and costs nothing. This PC has to be on with Chillarr running." }))
      : el("div", { class: "tip" }, el("span", { text: "Install Tailscale (free, tailscale.com) on this PC and on your phone, and sign in with the same account. Your Tailscale address shows up here, ready for the Jellyfin app, wherever you are." })));
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
      el("small", { text: "Search in every Jellyfin app (TV, phone, web) forgives typos and spacing: \"spiderman\", \"ironman\" and \"dune 2\" find the right title. Runs on this PC only." }),
      el("small", { class: "search-state", text: line })), sw));
  };
  invoke("meili_status").then((st) => draw(st)).catch(() => {});
  return box;
}

/** The Discover shelf and Chillarr's name in Jellyfin apps on TVs and phones. */
function shelfBlock() {
  const box = el("div", { class: "shelf-block" }, el("h2", { text: "TV and Jellyfin apps", style: "margin-top:36px;font-size:1.2rem" }));
  const draw = (st) => {
    const on = el("input", { type: "checkbox", class: "switch" });
    on.checked = st.on;
    on.addEventListener("input", async () => { on.disabled = true; draw(await invoke("shelf_set", { on: on.checked, seriesMode: null })); });
    const brand = el("input", { type: "checkbox", class: "switch" });
    brand.checked = st.brand;
    brand.addEventListener("input", async () => { brand.disabled = true; draw(await invoke("shelf_brand", { on: brand.checked })); });
    const seg = el("div", { class: "segmented", role: "group", "aria-label": "When a show is favourited" },
      [["season1", "Season 1"], ["all", "Whole show"]].map(([v, t]) => el("button", { class: st.series_mode === v ? "on" : "", onclick: async () => draw(await invoke("shelf_set", { on: st.on, seriesMode: v })) }, t)));
    const count = st.on
      ? `${st.movies} movies and ${st.series} series on the shelf${st.requested.length ? `. On their way: ${st.requested.slice(0, 4).join(", ")}${st.requested.length > 4 ? ` and ${st.requested.length - 4} more` : ""}.` : "."}`
      : "Off.";
    box.replaceChildren(box.firstChild,
      el("p", { class: "note", text: "Get new movies and series from any Jellyfin app, on a TV, phone or tablet. Open Discover Movies or Discover Series there and press the heart on anything: this PC downloads it, and it moves into Movies or Series when it's ready. On your home network, pressing play starts it and plays it after about a minute. Playing an episode gets that episode only." }),
      el("div", { class: "box" },
        el("label", { class: "opt" }, el("span", {}, el("b", { text: "Discover shelf" }), el("small", { text: count })), on),
        el("div", { class: "opt" }, el("span", {}, el("b", { text: "When a show is favourited" }), el("small", { text: "Get the first season, or every season. A heart on one episode gets just that episode." })), seg),
        el("label", { class: "opt" }, el("span", {}, el("b", { text: "Show as Chillarr" }), el("small", { text: "Jellyfin apps call this server Chillarr, with its colours and a short welcome on the sign-in screen." })), brand)));
  };
  invoke("shelf_status").then(draw).catch(() => {});
  return box;
}

/** Subtitle downloads: a free OpenSubtitles.com login, handed to Jellyfin's plugin.
 *  `name` is the one login from setup: made with the same name, only the password is left to type. */
async function subtitleBlock(name) {
  const st = await invoke("subs_status").catch(() => ({ installed: false, ready: false }));
  const box = el("div", {}, el("h2", { text: "Subtitle downloads", style: "margin-top:36px;font-size:1.2rem" }));
  if (st.ready) {
    box.append(el("div", { class: "box" }, el("div", { class: "opt" },
      el("span", {}, el("b", { text: `On, as ${st.user}` }), el("small", { text: "While watching, open the tracks menu and pick Download subtitles. Jellyfin apps on TVs and phones also get English subtitles fetched for anything missing them." })))));
    return box;
  }
  const user = el("input", { type: "text", autocomplete: "username", value: name || "" });
  const pass = el("input", { type: "password", autocomplete: "current-password" });
  const err = el("p", { class: "err" });
  const go = el("button", { class: "btn primary" }, "Turn on");
  go.addEventListener("click", async () => {
    if (!user.value.trim() || !pass.value) { err.textContent = "Enter your OpenSubtitles.com name and password."; return; }
    go.disabled = true; err.textContent = "";
    go.textContent = st.installed ? "Checking..." : "Setting up (Jellyfin restarts once)...";
    try { toast(await invoke("subs_setup", { username: user.value.trim(), password: pass.value })); renderSettings($("#view"), "account"); }
    catch (e) { err.textContent = String(e); go.disabled = false; go.textContent = "Turn on"; }
  });
  box.append(el("p", { class: "note", text: "Find subtitles for anything in your library, in any language. It uses a free OpenSubtitles.com account (make one on their site, with the same name and password as Chillarr to keep it simple), and your name and password stay inside Jellyfin. Without it, the subtitles you pick on Download still come from free sources, just fewer of them." }),
    el("div", { style: "display:grid;gap:14px;max-width:420px;margin-top:14px" },
      el("label", { class: "field" }, el("small", { text: "OpenSubtitles.com name" }), user), el("label", { class: "field" }, el("small", { text: "Password" }), pass), el("div", {}, go), err));
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
    el("div", {}, ORDER.map((id) => {
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
  panel.append(el("h2", { text: "General" }), el("p", { class: "sub", text: "How Chillarr starts, plays and looks after things." }),
    el("div", { class: "box" },
      el("div", { class: "opt" }, el("span", {}, el("b", { text: "Play with" }), el("small", { text: vlc ? "VLC plays the original file at full quality with every audio track. Chillarr's player converts what it can't play directly." : "Install VLC to play original files at full quality." })), seg),
      sw("start_with_windows", "Start with Windows", "Opens in the tray when you sign in."),
      sw("auto_fix", "Fix automatically", "Checks every minute and fixes what breaks."),
      sw("defender_ask", "Defender access", "If Defender's folder protection blocks your media apps, Fix everything asks to let them through."),
      sw("skip_dubs", "Skip dubbed copies", "Leaves out copies with an extra dubbed language mixed in, like Hindi or Italian, so nothing starts in the wrong language. A film's own language is always fine, and so is anime with Japanese and English."),
      sw("skip_remux", "Skip full-disc copies", "Remux and Blu-ray disc copies are 25 to 80 GB for one film and look the same on a TV. With this on, every download takes a normal copy of a few GB instead, and movies over about 8 GB (1080p) are skipped. Whole seasons are judged per episode, so they still come."),
      seedSwitch(),
      subSizeRow()),
    remoteBox(),
    el("div", { style: "margin-top:24px" }, el("button", { class: "btn ghost", onclick: () => openSetup() }, "Run the setup again")));
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
  const hint = el("small", { text: "Keep sharing finished downloads with others. Off stops each one the moment it's done and clears it from Downloads once it's in your library. While something downloads it always shares a little; that's how torrents work." });
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
      el("small", { text: "Use your phone as a remote: arrows, OK, back, play and pause, and typing to search. Works on the same Wi-Fi, or anywhere over Tailscale. Every TV has its own code, shown on the TV under Settings > Phone remote." })), sw)];
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
      el("small", { text: "Xbox, PlayStation and other controllers work wired or over Bluetooth: D-pad or stick to move, A to choose, B to go back, LB and RB to switch tabs. TV remotes and air mice that send arrow keys work too." }))));
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
      row("Sign out", "Goes back to Jellyfin's sign-in, where someone else can sign in.", out)));
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
async function logs(panel) {
  const list = el("ol", { class: "log" });
  const item = (l) => el("li", { dataset: { level: l.level } }, el("time", { text: l.at }), el("span", { text: l.text }));
  const all = await invoke("get_logs");
  list.replaceChildren(...all.map(item));
  panel.append(el("h2", { text: "Logs" }), el("p", { class: "sub", text: "What Chillarr did, newest first. API keys are hidden." }), list);
  unsub = store.on((w) => { if (w === "logs") list.prepend(item(store.logs[0])); });
}
