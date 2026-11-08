// The Read tab: manga, manhwa and manhua. Rows come from AniList; what you
// follow, where you are and the pages come from read.rs on the PC. Following
// a series asks every source for it (suwayomi.rs), shows the copies best
// first and downloads the one you pick; new chapters then come by themselves.
import { ask, el, icon, ICON, invoke, toast } from "./api.js";
import { openReader } from "./reader.js";

const KIND = { KR: "Manhwa", CN: "Manhua", TW: "Manhua", JP: "Manga" };
const kindOf = (c) => (c.format === "NOVEL" ? "Novel" : KIND[c.country] || "Comic");
const STATUS = { RELEASING: "Ongoing", FINISHED: "Finished", HIATUS: "On hiatus", CANCELLED: "Cancelled", NOT_YET_RELEASED: "Coming soon" };

let seq = 0;
let followedByAnilist = new Map();

/* ---------- cards and rows ---------- */

function cover(src, title) {
  const box = el("div", { class: "art" });
  if (src) {
    const img = el("img", { src, alt: "", loading: "lazy", decoding: "async" });
    img.addEventListener("load", () => img.classList.add("loaded"), { once: true });
    img.addEventListener("error", () => img.replaceWith(el("div", { class: "noart" }, el("strong", { text: title }))));
    box.append(img);
  } else box.append(el("div", { class: "noart" }, el("strong", { text: title })));
  return box;
}

/** A card for an AniList title, or for a series you follow (`shelf`). */
function card(c, shelf = null) {
  const box = cover(c.cover, c.title);
  box.append(el("span", { class: "badge", text: kindOf(c) }));
  if (shelf && shelf.unread) box.append(el("span", { class: "badge get read-new", text: `${shelf.unread} new` }));
  const sub = shelf ? shelf.resume_label || (shelf.finished ? "Finished" : shelf.downloaded ? "All read" : "Downloading") : c.year ? String(c.year) : "";
  const b = el("button", { class: "tile", "aria-label": c.title }, box,
    el("div", { class: "cap" }, el("strong", { text: c.title }), sub ? ` · ${sub}` : ""));
  b.addEventListener("click", () => (shelf ? openSeries({ key: shelf.key }) : openSeries({ card: c })));
  return b;
}

function row(id, title, cards) {
  const track = el("div", { class: "track" }, cards);
  const step = () => track.clientWidth * 0.85;
  return el("section", { class: "row", dataset: { row: id } },
    el("div", { class: "row-head" }, el("h2", { text: title }), el("span", { class: "count", text: String(cards.length) })),
    el("div", { class: "track-wrap" },
      el("button", { class: "arrow prev", "aria-label": "Scroll left", onclick: () => track.scrollBy({ left: -step(), behavior: "smooth" }) }, icon(ICON.left)),
      track,
      el("button", { class: "arrow next", "aria-label": "Scroll right", onclick: () => track.scrollBy({ left: step(), behavior: "smooth" }) }, icon(ICON.right))));
}

/** The shelf's facts in the AniList card's shape, for a card of a followed series. */
const asCard = (s, f = {}) => ({ anilist: s.anilist, title: s.title, cover: s.cover, country: s.country, status: s.status, format: f.format });

/* ---------- the tab ---------- */

export async function renderRead(view) {
  const my = ++seq;
  const search = el("input", { class: "read-search", type: "search", placeholder: "Search manga, manhwa, manhua", "aria-label": "Search reading", autocomplete: "off" });
  const results = el("div", { class: "read-results" });
  const lib = el("div");
  const rows = el("div", {}, el("div", { class: "skel skel-row" }), el("div", { class: "skel skel-row" }));
  view.replaceChildren(el("div", { class: "read-page" },
    el("header", { class: "read-head" }, el("h1", { text: "Read" }), search), results, lib, rows));
  let t = 0;
  let asked = 0;
  search.addEventListener("input", () => {
    clearTimeout(t);
    const term = search.value.trim();
    if (!term) { results.replaceChildren(); return; }
    t = setTimeout(async () => {
      const mine = ++asked;
      const found = await invoke("read_search", { term }).catch((e) => { toast(String(e), true); return []; });
      if (mine !== asked || my !== seq) return;
      results.replaceChildren(found.length ? row("search", `Results for "${term}"`, found.map((c) => card(c))) : el("p", { class: "note", text: "Nothing found." }));
    }, 350);
  });
  fillLibrary(lib, my);
  try {
    const got = await invoke("read_rows");
    if (my !== seq) return;
    rows.replaceChildren(...got.filter((r) => r.items.length).map((r) => row(r.id, r.title, r.items.map((c) => card(c)))));
  } catch (e) {
    if (my !== seq) return;
    rows.replaceChildren(el("p", { class: "note", text: String(e) }));
  }
  return true;
}

async function fillLibrary(slot, my) {
  const list = await invoke("read_library").catch(() => []);
  if (my !== seq) return;
  followedByAnilist = new Map(list.filter((s) => s.anilist).map((s) => [s.anilist, s.key]));
  const reading = list.filter((s) => !s.finished);
  const done = list.filter((s) => s.finished);
  const out = [];
  if (reading.length) out.push(row("reading", "Continue reading", reading.map((s) => card(asCard(s), s))));
  if (done.length) out.push(row("finished", "Finished", done.map((s) => card(asCard(s), s))));
  slot.replaceChildren(...out);
}

export const stopRead = () => { seq++; };

/* ---------- the series panel ---------- */

let dlg = null;
let panelSeq = 0;
let poll = 0;

function panel() {
  if (dlg) return dlg;
  dlg = el("dialog", { class: "detail read-detail", "aria-label": "Series" });
  dlg.addEventListener("close", () => { clearTimeout(poll); panelSeq++; });
  dlg.addEventListener("click", (e) => { if (e.target === dlg) dlg.close(); });
  document.body.append(dlg);
  return dlg;
}

function shell(c) {
  const d = panel();
  const css = (u) => `url("${String(u).replace(/"/g, "%22")}")`;
  const hero = el("div", { class: "detail-hero read-hero" + (c.banner ? "" : " no-banner") });
  if (c.banner) hero.style.backgroundImage = css(c.banner);
  else if (c.cover) hero.append(el("div", { class: "read-hero-blur", style: `background-image:${css(c.cover)}` }));
  const meta = el("p", { class: "meta" }, ...[kindOf(c), STATUS[c.status], c.chapters ? `${c.chapters} chapters` : null, c.score ? `${c.score}%` : null, ...(c.genres || []).slice(0, 3)]
    .filter(Boolean).flatMap((x, i) => (i ? [el("span", { class: "sep" }), el("span", { text: x })] : [el("span", { text: x })])));
  const actions = el("div", { class: "actions" });
  const poster = c.cover ? el("img", { class: "read-poster", src: c.cover, alt: "" }) : null;
  hero.append(el("button", { class: "close", "aria-label": "Close", onclick: () => d.close() }, icon("M6 6l12 12M18 6L6 18")),
    el("div", { class: "read-head-row" }, poster, el("div", { class: "detail-head" }, el("h2", { text: c.title }), meta, actions)));
  const body = el("div", { class: "detail-body" }, c.description ? el("p", { class: "overview read-overview", text: c.description }) : null);
  d.replaceChildren(hero, body);
  if (!d.open) d.showModal();
  return { actions, body };
}

/** Opens a series: `{ key }` for one you follow, `{ card }` for an AniList title. */
export async function openSeries({ key = null, card: c = null }) {
  const mine = ++panelSeq;
  clearTimeout(poll);
  if (!key && c?.anilist && followedByAnilist.has(c.anilist)) key = followedByAnilist.get(c.anilist);
  if (key) return showFollowed(key, mine, c);
  const { actions, body } = shell(c);
  const find = el("button", { class: "btn primary" }, icon(ICON.download), "Find copies");
  actions.append(find);
  const list = el("section", { class: "read-sources" });
  body.append(list);
  find.addEventListener("click", () => findCopies(c, list, find, mine));
}

/** Every source's copy, best first, each with its own Download. */
async function findCopies(c, list, btn, mine) {
  btn.disabled = true;
  btn.lastChild.textContent = "Looking...";
  list.replaceChildren(el("p", { class: "note read-wait", text: "Checking every source for the sharpest complete copy. This takes about 20 seconds (longer the first time, while the downloader sets itself up)." }));
  let found;
  try {
    found = await invoke("read_sources", { meta: meta(c) });
  } catch (e) {
    if (mine !== panelSeq) return;
    list.replaceChildren(el("p", { class: "note", text: String(e) }));
    btn.disabled = false;
    btn.lastChild.textContent = "Try again";
    return;
  }
  if (mine !== panelSeq) return;
  btn.remove();
  if (!found.length) { list.replaceChildren(el("p", { class: "note", text: "No source has this one yet." })); return; }
  const from = el("input", { class: "read-from", type: "number", min: "0", step: "1", placeholder: "1", "aria-label": "Start from chapter" });
  list.replaceChildren(
    el("div", { class: "read-sources-head" }, el("h3", { text: "Pick a copy" }),
      el("label", { class: "read-from-wrap" }, "From chapter ", from)),
    ...found.map((x, i) => sourceRow(c, x, i === 0, from, mine)));
}

function sourceRow(c, x, best, from, mine) {
  const facts = [
    `${x.chapters} chapter${x.chapters === 1 ? "" : "s"}`,
    x.latest != null ? `up to ${fmtNum(x.latest)}` : null,
    x.width ? `${x.width} px wide` : "page size unknown",
  ].filter(Boolean).join(" · ");
  const go = el("button", { class: best ? "btn primary" : "btn soft" }, icon(ICON.download), "Download");
  go.addEventListener("click", async () => {
    go.disabled = true;
    const n = from.value === "" ? null : Number(from.value);
    try {
      toast(await invoke("read_follow", { meta: meta(c), pick: x, from: n }));
      const lib = await invoke("read_library").catch(() => []);
      followedByAnilist = new Map(lib.filter((s) => s.anilist).map((s) => [s.anilist, s.key]));
      if (mine === panelSeq) openSeries({ key: `m${x.manga_id}`, card: c });
      window.dispatchEvent(new Event("chillarr:refresh"));
    } catch (e) { toast(String(e), true); go.disabled = false; }
  });
  return el("div", { class: "read-source" + (best ? " best" : "") },
    el("div", { class: "read-source-text" },
      el("strong", { text: x.source.replace(/ \((EN|ALL)\)$/, "") }), best ? el("span", { class: "read-best", text: "Best" }) : null,
      el("small", { text: facts }), x.title !== c.title ? el("small", { class: "read-alt", text: `Listed as "${x.title}"` }) : null),
    qualityBar(x.quality), go);
}

/** How sharp a copy looks, as a bar: 1600 px fills it. */
function qualityBar(q) {
  const pct = Math.max(4, Math.min(100, Math.round((q / 1600) * 100)));
  return el("div", { class: "read-q", title: "Page sharpness" }, el("span", { style: `width:${pct}%` }));
}

const fmtNum = (n) => (Number.isInteger(n) ? String(n) : String(n));
const meta = (c) => ({ anilist: c.anilist ?? null, title: c.title, names: c.names || [], cover: c.cover ?? null, banner: c.banner ?? null, country: c.country ?? null, format: c.format ?? null, status: c.status ?? null, description: c.description ?? null, genres: c.genres || [] });

async function showFollowed(key, mine, c0) {
  let d;
  try { d = await invoke("read_detail", { key }); } catch (e) { toast(String(e), true); return; }
  if (mine !== panelSeq) return;
  const f = d.followed;
  const c = { ...(c0 || {}), ...f, chapters: d.chapters.length };
  const { actions, body } = shell(c);
  const readIds = new Set(d.chapters.filter((r) => r.read).map((r) => r.id));
  const spot = d.spot;
  const resume = (spot && !readIds.has(spot.chapter) && d.chapters.find((r) => r.id === spot.chapter && r.downloaded))
    || d.chapters.slice(spot ? Math.max(0, d.chapters.findIndex((r) => r.id === spot.chapter)) : 0).find((r) => r.downloaded && !readIds.has(r.id))
    || d.chapters.find((r) => r.downloaded);
  if (resume) {
    const label = spot && spot.chapter === resume.id && spot.page ? `Resume ${chLabel(resume)}` : `Read ${chLabel(resume)}`;
    actions.append(el("button", { class: "btn play", onclick: () => read(key, resume.id, c0) }, icon(ICON.play), label));
  }
  actions.append(el("button", { class: "btn soft", onclick: () => unfollow(f) }, "Stop following"));
  const where = el("p", { class: "note read-from-source" },
    `From ${f.source.replace(/ \((EN|ALL)\)$/, "")}`, f.width ? ` · ${f.width} px wide` : "",
    d.downloading ? ` · getting ${d.downloading} chapter${d.downloading === 1 ? "" : "s"}` : "",
    " · ", el("button", { class: "linkish", onclick: () => changeSource(c, body, mine) }, "Change copy"));
  const list = el("ol", { class: "read-chapters", reversed: false },
    ...d.chapters.slice().reverse().map((r) => chapterRow(key, r, c0)));
  body.append(where, list);
  // Chapters still coming: the list keeps itself current.
  if (d.downloading) poll = setTimeout(() => { if (mine === panelSeq && dlg?.open) showFollowed(key, mine, c0); }, 5000);
}

const chLabel = (r) => (r.number != null ? `Chapter ${fmtNum(r.number)}` : r.name || "Chapter");

function chapterRow(key, r, c0) {
  const state = r.queued != null ? `Downloading ${Math.round(r.queued * 100)}%` : r.downloaded ? (r.read ? "Read" : r.progress ? `${Math.round(r.progress * 100)}%` : "") : "Not downloaded";
  const name = r.name && r.name !== chLabel(r) && !/^chapter\s*[\d.]+$/i.test(r.name) ? r.name : "";
  const li = el("li", { class: "read-ch" + (r.read ? " is-read" : "") + (r.downloaded ? "" : " not-here") },
    el("button", { class: "read-ch-open", disabled: !r.downloaded, onclick: () => read(key, r.id, c0) },
      el("strong", { text: chLabel(r) }), name ? el("span", { class: "read-ch-name", text: name }) : null,
      r.scanlator ? el("small", { text: r.scanlator }) : null),
    el("span", { class: "read-ch-state", text: state }),
    r.downloaded ? el("button", { class: "read-mark", title: r.read ? "Mark unread" : "Mark read", "aria-label": r.read ? "Mark unread" : "Mark read",
      onclick: async (e) => { e.stopPropagation(); try { await invoke("read_mark", { key, chapters: [r.id], read: !r.read }); refreshIfOpen(key, panelSeq); } catch (err) { toast(String(err), true); } } },
      icon(r.read ? "M5 12l5 5L20 7" : "M12 5a7 7 0 1 0 0 14 7 7 0 0 0 0-14z")) : null);
  return li;
}

/** The reader goes over everything: the panel (a modal, drawn above all
 *  else) closes first and comes back, up to date, when the reader closes. */
function read(key, chapter, c) {
  dlg?.close();
  openReader(key, chapter, () => { openSeries({ key, card: c }); window.dispatchEvent(new Event("chillarr:refresh")); });
}

function refreshIfOpen(key, mine) {
  if (dlg?.open && mine === panelSeq) showFollowed(key, ++panelSeq);
  window.dispatchEvent(new Event("chillarr:refresh"));
}

async function changeSource(c, body, mine) {
  const list = el("section", { class: "read-sources" });
  body.prepend(list);
  list.scrollIntoView({ behavior: "smooth", block: "start" });
  // findCopies sets the button's last text node; this one isn't shown.
  findCopies(c, list, el("button", {}, "Looking..."), mine);
}

async function unfollow(f) {
  if (!(await ask(`Stop following ${f.title}?`, "New chapters stop downloading.", [], "Stop following", "Cancel"))) return;
  const del = await ask("Delete its chapters too?", "Keep them to read later, or free the space now.", [], "Delete them", "Keep them");
  try {
    toast(await invoke("read_unfollow", { key: f.key, delete: del }));
    followedByAnilist.delete(f.anilist);
    dlg?.close();
    window.dispatchEvent(new Event("chillarr:refresh"));
  } catch (e) { toast(String(e), true); }
}

