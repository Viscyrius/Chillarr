// Reading mode: manga, manhwa, manhua, novels and books, a whole Chillarr
// of its own (the book button in the top bar switches; app.js routes the
// r-* tabs here). Rows come from AniList and Open Library; what you follow,
// your list, where you are and the pages come from read.rs on the PC, per
// profile. Getting a series: pick the translation, then Best (the sharpest
// complete copy, found by asking every source) or a copy of your choice,
// then which chapters.
import { ask, el, getPref, icon, ICON, invoke, setPref, toast } from "./api.js";
import { openReader } from "./reader.js";
import { openBook } from "./bookreader.js";

const KIND = { KR: "Manhwa", CN: "Manhua", TW: "Manhua", JP: "Manga" };
const kindOf = (c) => (c.kind === "book" ? "Book" : c.format === "NOVEL" ? "Light novel" : KIND[c.country] || "Comic");
/** Books and light novels open the book panel; the rest are comics. */
const isBook = (c) => c.kind === "book" || c.format === "NOVEL";
const STATUS = { RELEASING: "Ongoing", FINISHED: "Finished", HIATUS: "On hiatus", CANCELLED: "Cancelled", NOT_YET_RELEASED: "Coming soon" };
const TAB_TITLE = { "r-manga": "Manga", "r-manhwa": "Manhwa", "r-manhua": "Manhua", "r-novels": "Light novels" };

let seq = 0;
let followedByAnilist = new Map();
let followedLang = new Map();
let listKeys = new Set();
let myBooks = [];

/* ---------- small pieces ---------- */

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

const cardKey = (c) => (c.anilist ? `al:${c.anilist}` : c.olid ? `ol:${c.olid}` : "");

/** A card for an AniList or Open Library title, or a series you follow (`shelf`). */
function card(c, shelf = null) {
  const box = cover(c.cover, c.title);
  box.append(el("span", { class: "badge", text: kindOf(c) }));
  // The translation it's read in, on the cover, like a season on a show.
  const lang = shelf ? shelf.lang : c.anilist && followedLang.get(c.anilist);
  if (lang) box.append(el("span", { class: "badge read-lang", text: lang.toUpperCase() }));
  if (shelf && shelf.unread) box.append(el("span", { class: "badge get read-new", text: `${shelf.unread} new` }));
  const sub = shelf ? shelf.resume_label || (shelf.finished ? "Finished" : shelf.downloaded ? "All read" : "Downloading") : c.author || (c.year ? String(c.year) : "");
  const b = el("button", { class: "tile", "aria-label": c.title }, box,
    el("div", { class: "cap" }, el("strong", { text: c.title }), sub ? ` · ${sub}` : ""));
  b.addEventListener("click", () => (shelf ? openSeries({ key: shelf.key }) : isBook(c) ? openBookPanel(c) : openSeries({ card: c })));
  return b;
}

/** A card for a book on your list. */
function bookCard(x) {
  const box = cover(x.cover, x.title);
  box.append(el("span", { class: "badge", text: "Book" }));
  if (x.read > 0 && x.read < 1) box.append(el("div", { class: "bar" }, el("span", { style: `width:${Math.round(x.read * 100)}%` })));
  const sub = x.problem ? "Needs a look" : !x.ready ? (x.progress != null ? `Downloading ${Math.round(x.progress * 100)}%` : "Starting") : x.read >= 1 ? "Finished" : x.read > 0 ? `${Math.round(x.read * 100)}% read` : x.author || "";
  const b = el("button", { class: "tile", "aria-label": x.title }, box, el("div", { class: "cap" }, el("strong", { text: x.title }), sub ? ` · ${sub}` : ""));
  b.addEventListener("click", () => openBookPanel({ kind: "book", title: x.title, author: x.author, cover: x.cover, description: x.description, olid: x.olid, anilist: x.anilist }, x));
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

const asCard = (s) => ({ anilist: s.anilist, title: s.title, cover: s.cover, country: s.country, status: s.status });
const page = (...kids) => el("div", { class: "read-page" }, ...kids);
const head = (title, ...rest) => el("header", { class: "read-head" }, el("h1", { text: title }), ...rest);
const skel = () => [el("div", { class: "skel skel-row" }), el("div", { class: "skel skel-row" })];
const note = (text) => el("p", { class: "note", text });

async function know() {
  const [lib, list] = await Promise.all([invoke("read_library").catch(() => []), invoke("read_list").catch(() => [])]);
  followedByAnilist = new Map(lib.filter((s) => s.anilist).map((s) => [s.anilist, s.key]));
  followedLang = new Map(lib.filter((s) => s.anilist && s.lang).map((s) => [s.anilist, s.lang]));
  listKeys = new Set(list.map(cardKey));
  return { lib, list };
}

/* ---------- the tabs ---------- */

/** Draws one reading tab: r-home, r-manga, r-manhwa, r-manhua, r-novels, r-books, r-mylist, r-downloads. */
export async function renderRead(view, tab) {
  const my = ++seq;
  if (TAB_TITLE[tab]) return explorePage(view, tab, my);
  if (tab === "r-books") return booksPage(view, my);
  if (tab === "r-mylist") return listPage(view, my);
  if (tab === "r-downloads") return downloadsPage(view, my);
  return homePage(view, my);
}

export const stopRead = () => { seq++; };

async function homePage(view, my) {
  const lib = el("div");
  const rows = el("div", {}, ...skel());
  view.replaceChildren(page(head("Reading"), lib, rows));
  const [{ lib: series }, books] = await Promise.all([know(), invoke("read_books").catch(() => [])]);
  if (my !== seq) return;
  myBooks = books;
  const reading = series.filter((s) => !s.finished);
  const done = series.filter((s) => s.finished);
  const out = [];
  if (reading.length) out.push(row("reading", "Continue reading", reading.map((s) => card(asCard(s), s))));
  if (books.filter((b) => b.ready && b.read < 1).length) out.push(row("books", "Your books", books.filter((b) => b.read < 1).map(bookCard)));
  if (done.length) out.push(row("finished", "Finished", done.map((s) => card(asCard(s), s))));
  lib.replaceChildren(...out);
  try {
    const got = await invoke("read_rows");
    if (my !== seq) return;
    rows.replaceChildren(...got.filter((r) => r.items.length).map((r) => row(r.id, r.title, r.items.map((c) => card(c)))));
    invoke("read_book_rows").then((bk) => {
      if (my !== seq) return;
      rows.append(...bk.filter((r) => r.items.length).slice(0, 2).map((r) => row(r.id, r.title, r.items.map((c) => card(c)))));
    }).catch(() => {});
  } catch (e) {
    if (my === seq) rows.replaceChildren(note(String(e)));
  }
  return true;
}

/** Manga, Manhwa, Manhua, Light novels: a grid with filters, more as you scroll. */
async function explorePage(view, tab, my) {
  const kind = tab.slice(2);
  const saved = (() => { try { return JSON.parse(getPref(`read-filter-${kind}`) || "{}"); } catch (e) { return {}; } })();
  const f = { genres: saved.genres || [], status: saved.status || null, sort: saved.sort || "popular" };
  const genres = await invoke("read_genres").catch(() => []);
  if (my !== seq) return;
  const grid = el("div", { class: "grid read-grid" });
  const more = el("div", { class: "read-more" });
  const chips = el("div", { class: "chips read-chips" });
  const select = (label, value, options, set) => {
    const s = el("select", { class: "quality", "aria-label": label }, options.map(([v, t]) => el("option", { value: v, text: t, selected: (value || "") === v })));
    s.addEventListener("change", () => { set(s.value || null); restart(); });
    return s;
  };
  const drawChips = () => chips.replaceChildren(...genres.map((g) => {
    const on = f.genres.includes(g);
    return el("button", { class: "chip" + (on ? " on" : ""), "aria-pressed": on ? "true" : "false", onclick: () => {
      f.genres = on ? f.genres.filter((x) => x !== g) : [...f.genres, g];
      drawChips(); restart();
    } }, g);
  }));
  drawChips();
  const bar = el("div", { class: "read-filters" },
    select("Status", f.status, [["", "Any status"], ["RELEASING", "Ongoing"], ["FINISHED", "Finished"], ["HIATUS", "On hiatus"]], (v) => (f.status = v)),
    select("Order", f.sort, [["popular", "Most popular"], ["trending", "Trending"], ["score", "Top rated"], ["new", "Newest"], ["title", "A to Z"]], (v) => (f.sort = v || "popular")));
  view.replaceChildren(page(head(TAB_TITLE[tab], bar), chips, grid, more));
  await know();
  let pageNo = 0, done = false, busy = false, run = 0;
  async function next() {
    if (busy || done || my !== seq) return;
    busy = true;
    const mine = run;
    more.replaceChildren(el("div", { class: "skel skel-row" }));
    try {
      const got = await invoke("read_explore", { e: { kind, genres: f.genres, status: f.status, sort: f.sort, page: ++pageNo } });
      if (my !== seq || mine !== run) return;
      grid.append(...(got.items || []).map((c) => card(c)));
      done = !got.more;
      more.replaceChildren(done ? note(grid.childElementCount ? "That's everything." : "Nothing matches these filters.") : "");
      // Still at the bottom (a tall screen, or the page came in while it was
      // already showing): the watcher won't fire again, so ask for the next now.
      if (!done) setTimeout(() => { if (my === seq && mine === run && more.getBoundingClientRect().top < innerHeight + 900) next(); }, 0);
    } catch (e) {
      if (my === seq && mine === run) { more.replaceChildren(note(String(e))); done = true; }
    } finally { if (mine === run) busy = false; }
  }
  function restart() {
    setPref(`read-filter-${kind}`, JSON.stringify(f));
    run++; pageNo = 0; done = false; busy = false;
    grid.replaceChildren();
    next();
  }
  const io = new IntersectionObserver((e) => { if (e[0].isIntersecting) next(); }, { rootMargin: "900px" });
  io.observe(more);
  const stop = () => { if (my !== seq) io.disconnect(); };
  window.addEventListener("hashchange", stop, { once: true });
  await next();
  return true;
}

async function booksPage(view, my) {
  const mine = el("div");
  const rows = el("div", {}, ...skel());
  view.replaceChildren(page(head("Books"), mine, rows));
  myBooks = await invoke("read_books").catch(() => []);
  if (my !== seq) return;
  if (myBooks.length) mine.replaceChildren(row("books", "Your books", myBooks.map(bookCard)));
  const bk = await invoke("read_book_rows").catch(() => []);
  if (my !== seq) return;
  rows.replaceChildren(...(bk.filter((r) => r.items.length).map((r) => row(r.id, r.title, r.items.map((c) => card(c))))));
  if (!rows.childElementCount) rows.replaceChildren(note("Open Library isn't answering. Try again in a minute."));
  return true;
}

async function listPage(view, my) {
  const body = el("div", {}, ...skel());
  view.replaceChildren(page(head("My List"), body));
  const [{ lib, list }, books] = await Promise.all([know(), invoke("read_books").catch(() => [])]);
  if (my !== seq) return;
  const out = [];
  if (list.length) out.push(row("saved", "Saved for later", list.map((c) => card(c))));
  if (lib.length) out.push(row("following", "Following", lib.map((s) => card(asCard(s), s))));
  if (books.length) out.push(row("books", "Books", books.map(bookCard)));
  body.replaceChildren(...(out.length ? out : [note("Nothing here yet. Press Add to My List on anything you'd like to read later.")]));
  return true;
}

async function downloadsPage(view, my) {
  const body = el("div", {}, ...skel());
  view.replaceChildren(page(head("Downloads"), body));
  const draw = async () => {
    const d = await invoke("read_downloads").catch((e) => ({ error: String(e), series: [], books: [] }));
    if (my !== seq) return;
    const items = [
      ...d.series.map((s) => el("div", { class: "read-dl" },
        el("img", { class: "read-dl-cover", src: s.cover || "", alt: "" }),
        el("div", { class: "read-dl-text" }, el("strong", { text: s.title }),
          el("small", { text: `${s.chapters} chapter${s.chapters === 1 ? "" : "s"} to go` }),
          el("div", { class: "read-q wide" }, el("span", { style: `width:${Math.max(3, Math.round(s.progress * 100))}%` }))),
        el("button", { class: "btn soft small", onclick: async () => { try { toast(await invoke("read_stop_chapters", { key: s.key })); draw(); } catch (e) { toast(String(e), true); } } }, "Stop"))),
      ...d.books.map((b) => el("div", { class: "read-dl" },
        el("img", { class: "read-dl-cover", src: b.cover || "", alt: "" }),
        el("div", { class: "read-dl-text" }, el("strong", { text: b.title }),
          el("small", { text: b.problem || (b.progress != null ? `${Math.round(b.progress * 100)}%` : "Starting") }),
          el("div", { class: "read-q wide" }, el("span", { style: `width:${Math.max(3, Math.round((b.progress || 0) * 100))}%` }))))),
    ];
    body.replaceChildren(...(items.length ? items : [note(d.error || "Nothing is downloading.")]));
    if (items.length) setTimeout(() => { if (my === seq) draw(); }, 4000);
  };
  await draw();
  return true;
}

/** The top bar's search, in reading mode. */
export async function renderReadSearch(view, term) {
  const my = ++seq;
  const body = el("div", {}, ...skel());
  view.replaceChildren(page(head(`"${term}"`), body));
  const [found, books] = (await Promise.all([
    invoke("read_search", { term }).catch(() => []),
    invoke("read_book_search", { term }).catch(() => []),
  ])).map((x) => x || []);
  if (my !== seq) return;
  await know();
  const out = [];
  if (found.length) out.push(el("h2", { class: "read-grid-title", text: "Manga, manhwa, manhua and novels" }), el("div", { class: "grid read-grid" }, found.map((c) => card(c))));
  if (books.length) out.push(el("h2", { class: "read-grid-title", text: "Books" }), el("div", { class: "grid read-grid" }, books.map((c) => card(c))));
  body.replaceChildren(...(out.length ? out : [note("Nothing found.")]));
}

/* ---------- the series panel ---------- */

let dlg = null;
let panelSeq = 0;
let poll = 0;
let langs = null;

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
  const options = el("div", { class: "read-options" });
  const actions = el("div", { class: "actions" });
  const poster = c.cover ? el("img", { class: "read-poster", src: c.cover, alt: "" }) : null;
  hero.append(el("button", { class: "close", "aria-label": "Close", onclick: () => d.close() }, icon("M6 6l12 12M18 6L6 18")),
    el("div", { class: "read-head-row" }, poster, el("div", { class: "detail-head" }, el("h2", { text: c.title }), meta, options, actions)));
  const body = el("div", { class: "detail-body" }, c.description ? el("p", { class: "overview read-overview", text: c.description }) : null);
  d.replaceChildren(hero, body);
  if (!d.open) d.showModal();
  return { actions, body, options };
}

/** My List on any title: a toggle that remembers per profile. */
function listButton(c) {
  const key = cardKey(c);
  if (!key) return null;
  const b = el("button", { class: "btn soft" });
  const draw = () => b.replaceChildren(icon(listKeys.has(key) ? "M5 12l5 5L20 7" : "M12 5v14M5 12h14"), listKeys.has(key) ? "On My List" : "Add to My List");
  draw();
  b.addEventListener("click", async () => {
    const on = !listKeys.has(key);
    try {
      await invoke("read_list_set", { card: c, on });
      if (on) listKeys.add(key); else listKeys.delete(key);
      draw();
      toast(on ? `Added ${c.title} to My List.` : `Took ${c.title} off My List.`);
    } catch (e) { toast(String(e), true); }
  });
  return b;
}

async function languages() {
  if (!langs) langs = invoke("read_languages").catch((e) => { langs = null; throw e; });
  return langs;
}

/** The translation and copy pickers on a series not followed yet. */
function pickers(options, state) {
  const lang = el("select", { class: "quality read-lang-pick", "aria-label": "Translation" },
    el("option", { value: state.lang, text: state.lang === "en" ? "English" : state.lang }));
  languages().then((list) => {
    if (!list.length) return;
    lang.replaceChildren(...list.map((l) => el("option", { value: l.code, text: l.name, selected: l.code === state.lang })));
  }).catch(() => {});
  lang.addEventListener("change", () => { state.lang = lang.value; setPref("read-lang", lang.value); state.onChange(); });
  const copy = el("select", { class: "quality", "aria-label": "Copy" },
    el("option", { value: "best", text: "Best quality", selected: state.copy === "best" }),
    el("option", { value: "choose", text: "Let me choose", selected: state.copy === "choose" }));
  copy.addEventListener("change", () => { state.copy = copy.value; setPref("read-copy", copy.value); state.onChange(); });
  options.append(el("label", {}, el("span", { text: "Translation" }), lang), el("label", {}, el("span", { text: "Copy" }), copy));
}

/** Opens a series: `{ key }` for one you follow, `{ card }` for a title. */
export async function openSeries({ key = null, card: c = null }) {
  const mine = ++panelSeq;
  clearTimeout(poll);
  if (!key && c && c.anilist && followedByAnilist.has(c.anilist)) key = followedByAnilist.get(c.anilist);
  if (key) return showFollowed(key, mine, c);
  const { actions, body, options } = shell(c);
  const state = { lang: getPref("read-lang") || "en", copy: getPref("read-copy") || "best", onChange: () => { list.replaceChildren(); go.disabled = false; } };
  pickers(options, state);
  const go = el("button", { class: "btn primary" }, icon(ICON.download), "Download");
  actions.append(go, listButton(c));
  const list = el("section", { class: "read-sources" });
  body.prepend(list);
  go.addEventListener("click", () => getCopies(c, state, list, go, mine));
}

/** Finds the copies in the chosen translation; Best goes straight to its chapters. */
async function getCopies(c, state, list, btn, mine) {
  btn.disabled = true;
  list.replaceChildren(el("p", { class: "note read-wait", text: "Checking every source for the sharpest complete copy. About 20 seconds." }));
  let got;
  try {
    got = await invoke("read_sources", { meta: meta(c), lang: state.lang });
  } catch (e) {
    if (mine !== panelSeq) return;
    list.replaceChildren(note(String(e)));
    btn.disabled = false;
    return;
  }
  if (mine !== panelSeq) return;
  const found = got.list;
  if (!found.length) {
    list.replaceChildren(note(state.lang === "en" ? "No source has this one yet." : "No copy in that language yet. Try English."));
    btn.disabled = false;
    return;
  }
  // Best: the sharpest copy, and the one furthest ahead filling in what it lacks.
  const best = found[got.best ?? 0];
  const fill = got.fill != null ? found[got.fill] : null;
  if (state.copy === "best") return pickChapters(c, best, state, list, mine, found, fill);
  chooseCopy(c, state, list, mine, found);
}

function chooseCopy(c, state, list, mine, found) {
  list.replaceChildren(el("div", { class: "read-sources-head" }, el("h3", { text: "Pick a copy" })),
    ...found.map((x, i) => sourceRow(c, x, i === 0, () => pickChapters(c, x, state, list, mine, found, null))));
}

function sourceRow(c, x, best, onPick) {
  const facts = [`${x.chapters} chapter${x.chapters === 1 ? "" : "s"}`, x.latest != null ? `up to ${x.latest}` : null, x.width ? `${x.width} px wide` : "page size unknown"].filter(Boolean).join(" · ");
  return el("div", { class: "read-source" + (best ? " best" : "") },
    el("div", { class: "read-source-text" },
      el("strong", { text: srcName(x.source) }), best ? el("span", { class: "read-best", text: "Best" }) : null,
      el("small", { text: facts }), x.title !== c.title ? el("small", { class: "read-alt", text: `Listed as "${x.title}"` }) : null),
    qualityBar(x.quality), el("button", { class: best ? "btn primary" : "btn soft", onclick: onPick }, "Pick chapters"));
}

const srcName = (s) => String(s || "").replace(/ \([A-Za-z-]+\)$/, "");

/** How sharp a copy looks, as a bar: 1600 px fills it. */
function qualityBar(q) {
  const pct = Math.max(4, Math.min(100, Math.round((q / 1600) * 100)));
  return el("div", { class: "read-q", title: "Page sharpness" }, el("span", { style: `width:${pct}%` }));
}

/** Every chapter of a copy, ticked or not: all by default, a range, or one by one. */
async function pickChapters(c, x, state, list, mine, found, fill) {
  list.replaceChildren(el("p", { class: "note read-wait", text: `Getting the chapter list from ${srcName(x.source)}...` }));
  let chs;
  try { chs = await invoke("read_copy_chapters", { mangaId: x.manga_id, fillId: fill ? fill.manga_id : null }); } catch (e) { if (mine === panelSeq) list.replaceChildren(note(String(e))); return; }
  if (mine !== panelSeq) return;
  const picked = new Set(chs.map((ch) => ch.id));
  let last = null;
  const count = el("strong");
  const goBtn = el("button", { class: "btn primary" }, icon(ICON.download), "");
  const newToo = el("input", { type: "checkbox", class: "switch", checked: true, "aria-label": "Also get new chapters" });
  const from = el("input", { class: "read-from", type: "number", min: "0", step: "1", placeholder: "1", "aria-label": "From chapter" });
  const rows = chs.map((ch, i) => {
    const box = el("input", { type: "checkbox", checked: true, "aria-label": chLabel(ch) });
    box.addEventListener("click", (e) => {
      // Shift-click ticks or unticks the whole stretch since the last one.
      if (e.shiftKey && last != null) {
        const [a, b] = [Math.min(last, i), Math.max(last, i)];
        for (let j = a; j <= b; j++) { set(j, box.checked); }
      } else set(i, box.checked);
      last = i;
      paint();
    });
    const r = el("label", { class: "read-pick" + (ch.downloaded ? " here" : "") }, box, el("span", { class: "read-pick-n", text: chLabel(ch) }),
      ch.name && !/^chapter\s*[\d.]+$/i.test(ch.name) ? el("span", { class: "read-pick-name", text: ch.name }) : null,
      ch.downloaded ? el("small", { text: "Here" }) : ch.fill && fill ? el("small", { text: srcName(fill.source) }) : null);
    r.box = box;
    return r;
  });
  function set(i, on) { rows[i].box.checked = on; if (on) picked.add(chs[i].id); else picked.delete(chs[i].id); }
  function paint() {
    const n = picked.size;
    count.textContent = `${n} of ${chs.length} chapters`;
    goBtn.lastChild.textContent = n === chs.length ? "Download all" : n ? `Download ${n}` : "Just follow";
  }
  from.addEventListener("input", () => {
    const f = from.value === "" ? null : Number(from.value);
    chs.forEach((ch, i) => set(i, f == null || (ch.number ?? Infinity) >= f));
    paint();
  });
  goBtn.addEventListener("click", async () => {
    goBtn.disabled = true;
    const all = picked.size === chs.length;
    const sel = { ids: all ? null : [...picked], from: null, new_too: newToo.checked };
    try {
      toast(await invoke("read_follow", { meta: meta(c), pick: x, fill, sel, lang: state.lang }));
      await know();
      if (mine === panelSeq) openSeries({ key: `m${x.manga_id}`, card: c });
      window.dispatchEvent(new Event("chillarr:refresh"));
    } catch (e) { toast(String(e), true); goBtn.disabled = false; }
  });
  const others = found.length > 1 ? el("button", { class: "linkish", onclick: () => chooseCopy(c, state, list, mine, found) }, "Other copies") : null;
  const filled = fill ? chs.filter((ch) => ch.fill).length : 0;
  list.replaceChildren(
    el("div", { class: "read-sources-head" },
      el("h3", {}, `From ${srcName(x.source)}`, x.width ? el("small", { class: "read-px", text: ` ${x.width} px` }) : null), others),
    filled ? note(`${srcName(x.source)} is sharpest but stops early, so ${filled} chapter${filled === 1 ? " comes" : "s come"} from ${srcName(fill.source)}, which is further ahead.`) : null,
    el("div", { class: "read-pick-bar" },
      el("label", { class: "read-from-wrap" }, "From chapter ", from),
      el("button", { class: "linkish", onclick: () => { chs.forEach((_, i) => set(i, true)); paint(); } }, "All"),
      el("button", { class: "linkish", onclick: () => { chs.forEach((_, i) => set(i, false)); paint(); } }, "None"),
      count),
    el("div", { class: "read-pick-list" }, rows.slice().reverse()),
    el("div", { class: "read-pick-go" }, el("label", { class: "read-newtoo" }, newToo, "Also get new chapters"), goBtn));
  paint();
}

const meta = (c) => ({ anilist: c.anilist ?? null, title: c.title, names: c.names || [], cover: c.cover ?? null, banner: c.banner ?? null, country: c.country ?? null, format: c.format ?? null, status: c.status ?? null, description: c.description ?? null, genres: c.genres || [] });
const chLabel = (r) => (r.number != null ? `Chapter ${r.number}` : r.name || "Chapter");

async function showFollowed(key, mine, c0) {
  let d;
  try { d = await invoke("read_detail", { key }); } catch (e) { toast(String(e), true); return; }
  if (mine !== panelSeq) return;
  const f = d.followed;
  const c = { ...(c0 || {}), ...f, chapters: d.chapters.length };
  const scroll = dlg && dlg.open ? dlg.scrollTop : 0;
  const { actions, body, options } = shell(c);
  if (f.lang) options.append(el("span", { class: "read-lang-tag", text: f.lang.toUpperCase() }));
  const readIds = new Set(d.chapters.filter((r) => r.read).map((r) => r.id));
  const spot = d.spot;
  const resume = (spot && !readIds.has(spot.chapter) && d.chapters.find((r) => r.id === spot.chapter && r.downloaded))
    || d.chapters.slice(spot ? Math.max(0, d.chapters.findIndex((r) => r.id === spot.chapter)) : 0).find((r) => r.downloaded && !readIds.has(r.id))
    || d.chapters.find((r) => r.downloaded);
  if (resume) {
    const label = spot && spot.chapter === resume.id && spot.page ? `Resume ${chLabel(resume)}` : `Read ${chLabel(resume)}`;
    actions.append(el("button", { class: "btn play", onclick: () => read(key, resume.id, c0) }, icon(ICON.play), label));
  }
  const missing = d.chapters.filter((r) => !r.downloaded && r.queued == null);
  if (missing.length) actions.append(el("button", { class: "btn soft", onclick: async (e) => {
    e.currentTarget.disabled = true;
    try { toast(await invoke("read_get_chapters", { key, ids: missing.map((r) => r.id) })); showFollowed(key, ++panelSeq, c0); } catch (err) { toast(String(err), true); }
  } }, icon(ICON.download), `Get the other ${missing.length}`));
  actions.append(el("button", { class: "btn soft", onclick: () => unfollow(f) }, "Stop following"));
  const where = el("p", { class: "note read-from-source" },
    `From ${srcName(f.source)}`, f.width ? ` · ${f.width} px wide` : "",
    f.fill && f.fill.length ? ` · the rest from ${f.fill.map((x) => srcName(x.source)).join(", ")}` : "",
    d.downloading ? ` · getting ${d.downloading} chapter${d.downloading === 1 ? "" : "s"}` : "",
    f.new_too === false ? " · new chapters off" : "",
    " · ", el("button", { class: "linkish", onclick: () => changeCopy(c, f, body) }, "Change copy"));
  const list = el("ol", { class: "read-chapters" }, ...d.chapters.slice().reverse().map((r) => chapterRow(key, r, c0)));
  body.append(where, list);
  if (scroll) dlg.scrollTop = scroll;
  // Chapters still coming: the list keeps itself current.
  if (d.downloading) poll = setTimeout(() => { if (mine === panelSeq && dlg && dlg.open) showFollowed(key, ++panelSeq, c0); }, 5000);
}

function chapterRow(key, r, c0) {
  const state = r.queued != null ? `Downloading ${Math.round(r.queued * 100)}%` : r.downloaded ? (r.read ? "Read" : r.progress ? `${Math.round(r.progress * 100)}%` : "") : "";
  const name = r.name && r.name !== chLabel(r) && !/^chapter\s*[\d.]+$/i.test(r.name) ? r.name : "";
  const get = !r.downloaded && r.queued == null
    ? el("button", { class: "read-mark", title: "Download this chapter", "aria-label": `Download ${chLabel(r)}`, onclick: async (e) => {
      e.stopPropagation();
      e.currentTarget.disabled = true;
      try { toast(await invoke("read_get_chapters", { key, ids: [r.id] })); refreshIfOpen(key, c0); } catch (err) { toast(String(err), true); }
    } }, icon(ICON.download))
    : null;
  const mark = r.downloaded ? el("button", { class: "read-mark", title: r.read ? "Mark unread" : "Mark read", "aria-label": r.read ? "Mark unread" : "Mark read",
    onclick: async (e) => { e.stopPropagation(); try { await invoke("read_mark", { key, chapters: [r.id], read: !r.read }); refreshIfOpen(key, c0); } catch (err) { toast(String(err), true); } } },
    icon(r.read ? "M5 12l5 5L20 7" : "M12 5a7 7 0 1 0 0 14 7 7 0 0 0 0-14z")) : null;
  return el("li", { class: "read-ch" + (r.read ? " is-read" : "") + (r.downloaded ? "" : " not-here") },
    el("button", { class: "read-ch-open", disabled: !r.downloaded, onclick: () => read(key, r.id, c0) },
      el("strong", { text: chLabel(r) }), name ? el("span", { class: "read-ch-name", text: name }) : null,
      r.scanlator ? el("small", { text: r.scanlator }) : null),
    el("span", { class: "read-ch-state", text: state }), get || mark);
}

/** The reader goes over everything: the panel (a modal, drawn above all
 *  else) closes first and comes back, up to date, when the reader closes. */
function read(key, chapter, c) {
  if (dlg) dlg.close();
  openReader(key, chapter, () => { openSeries({ key, card: c }); window.dispatchEvent(new Event("chillarr:refresh")); });
}

function refreshIfOpen(key, c0) {
  if (dlg && dlg.open) showFollowed(key, ++panelSeq, c0);
  window.dispatchEvent(new Event("chillarr:refresh"));
}

/** Another copy of a series you follow (another site or translation); what you've read stays. */
function changeCopy(c, f, body) {
  const mine = panelSeq;
  const list = el("section", { class: "read-sources" });
  const options = el("div", { class: "read-options" });
  const state = { lang: f.lang || getPref("read-lang") || "en", copy: "choose", onChange: () => getCopies(c, state, list, el("button"), mine) };
  pickers(options, state);
  body.prepend(options, list);
  list.scrollIntoView({ behavior: "smooth", block: "start" });
  getCopies(c, state, list, el("button"), mine);
}

async function unfollow(f) {
  if (!(await ask(`Stop following ${f.title}?`, "New chapters stop downloading.", [], "Stop following", "Cancel"))) return;
  const del = await ask("Delete its chapters too?", "Keep them to read later, or free the space now.", [], "Delete them", "Keep them");
  try {
    toast(await invoke("read_unfollow", { key: f.key, delete: del }));
    followedByAnilist.delete(f.anilist);
    if (dlg) dlg.close();
    window.dispatchEvent(new Event("chillarr:refresh"));
  } catch (e) { toast(String(e), true); }
}

/* ---------- the book panel ---------- */

const sameBook = (c, x) => (c.olid && x.olid === c.olid) || (c.anilist && x.anilist === c.anilist) || (x.title || "").toLowerCase() === (c.title || "").toLowerCase();
const bookMeta = (c) => ({ title: c.title, author: c.author ?? null, cover: c.cover ?? null, description: c.description ?? null, olid: c.olid ?? null, anilist: c.anilist ?? null });

/** A book or light novel: read it if it's here, else find a copy. */
export async function openBookPanel(c, mine = null) {
  const my = ++panelSeq;
  clearTimeout(poll);
  if (!mine) {
    myBooks = await invoke("read_books").catch(() => myBooks);
    if (my !== panelSeq) return;
    mine = myBooks.find((x) => sameBook(c, x)) || null;
  }
  const shown = { ...c, chapters: null, status: null, genres: c.genres || [] };
  const { actions, body } = shell(shown);
  if (c.author) actions.parentElement.querySelector(".meta").prepend(el("span", { text: c.author }), el("span", { class: "sep" }));
  if (mine) {
    if (mine.ready) {
      actions.append(el("button", { class: "btn play", onclick: () => { if (dlg) dlg.close(); openBook(mine.key, () => openBookPanel(c)); } }, icon(ICON.play),
        mine.read > 0 && mine.read < 1 ? `Resume (${Math.round(mine.read * 100)}%)` : mine.read >= 1 ? "Read again" : "Read"));
    } else {
      actions.append(el("span", { class: "status-pill", text: mine.problem || (mine.progress != null ? `Downloading ${Math.round(mine.progress * 100)}%` : "Starting the download") }));
      poll = setTimeout(() => { if (my === panelSeq && dlg && dlg.open) openBookPanel(c); }, 4000);
    }
    actions.append(el("button", { class: "btn soft", onclick: () => removeBook(mine) }, "Remove"));
    if (mine.problem) { const list = el("section", { class: "read-sources" }); body.append(list); bookCopies(c, list, el("button", {}, "x"), my); }
    return;
  }
  const find = el("button", { class: "btn primary" }, icon(ICON.download), "Find copies");
  actions.append(find, listButton(c));
  const list = el("section", { class: "read-sources" });
  body.prepend(list);
  find.addEventListener("click", () => bookCopies(c, list, find, my));
}

async function bookCopies(c, list, btn, my) {
  btn.disabled = true;
  list.replaceChildren(el("p", { class: "note read-wait", text: "Searching the download sites..." }));
  let found;
  try { found = await invoke("read_book_copies", { meta: bookMeta(c) }); }
  catch (e) { if (my !== panelSeq) return; list.replaceChildren(note(String(e))); btn.disabled = false; return; }
  if (my !== panelSeq) return;
  btn.remove();
  if (!found.length) { list.replaceChildren(note("No copy of this book was found. Try again another day, or search with another title.")); return; }
  const mb = (n) => (n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`);
  list.replaceChildren(el("div", { class: "read-sources-head" }, el("h3", { text: "Pick a copy" })),
    ...found.slice(0, 8).map((x, i) => {
      const go = el("button", { class: i === 0 ? "btn primary" : "btn soft" }, icon(ICON.download), "Download");
      go.addEventListener("click", async () => {
        go.disabled = true;
        try { toast(await invoke("read_book_get", { meta: bookMeta(c), id: x.id })); openBookPanel(c); window.dispatchEvent(new Event("chillarr:refresh")); }
        catch (e) { toast(String(e), true); go.disabled = false; }
      });
      return el("div", { class: "read-source" + (i === 0 ? " best" : "") },
        el("div", { class: "read-source-text" }, el("strong", { text: (x.format || "ebook").toUpperCase() }), i === 0 ? el("span", { class: "read-best", text: "Best" }) : null,
          el("small", { text: `${mb(x.size)} · ${x.seeders} sharing · ${x.indexer}` }), el("small", { class: "read-alt", text: x.title })), go);
    }));
}

async function removeBook(x) {
  if (!(await ask(`Remove ${x.title}?`, "The book's file is deleted from your PC. You can download it again.", [], "Remove", "Cancel"))) return;
  try { toast(await invoke("read_book_remove", { key: x.key })); if (dlg) dlg.close(); window.dispatchEvent(new Event("chillarr:refresh")); }
  catch (e) { toast(String(e), true); }
}
