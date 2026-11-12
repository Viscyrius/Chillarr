// Reading mode: manga, manhwa, manhua, novels and books, a whole Chillarr
// of its own (the book button in the top bar switches; app.js routes the
// r-* tabs here). Rows come from AniList and Open Library; what you follow,
// your list, where you are and the pages come from read.rs on the PC, per
// profile. Getting a series: pick the translation, then Best (the sharpest
// complete copy, found by asking every source) or a copy of your choice,
// then which chapters.
import { hideButton, isHidden, markHidden } from "./hidden.js";
import { ask, choose, el, expandRow, getPref, icon, ICON, invoke, listen, personKey, setPref, toast } from "./api.js";
import { selectable, stopSelecting } from "./select.js";
import { openReader } from "./reader.js";
import { openBook } from "./bookreader.js";
import { adultDefault, adultFields, adultOf, adultShows, chipKit, pornOn, clearButton, filtersButton, moreButton, orderSelect } from "./filterpanel.js";
import { notifyButton } from "./notify.js";
import { applyLook, drawPickers, inlinePicker, onStyle, styleNow } from "./look.js";

const KIND = { KR: "Manhwa", CN: "Manhua", TW: "Manhua", JP: "Manga" };
const kindOf = (c) => (c.kind === "book" ? "Book" : c.kind === "comic" ? "Comic" : c.format === "NOVEL" ? "Light novel" : KIND[c.country] || "Comic");
/** Books, light novels and Western comics (got as one file, like a book) open the book panel. */
const isBook = (c) => c.kind === "book" || c.kind === "comic" || c.format === "NOVEL";
const STATUS = { RELEASING: "Ongoing", FINISHED: "Finished", HIATUS: "On hiatus", CANCELLED: "Cancelled", NOT_YET_RELEASED: "Coming soon" };
const TAB_TITLE = { "r-manga": "Manga", "r-manhwa": "Manhwa", "r-manhua": "Manhua", "r-novels": "Light novels" };

let seq = 0;
// A kept page put back on screen (renderRead) is current again under its old
// number, so its loaders (a grid's next page) carry on where they were.
let alias = -1;
const alive = (m) => m === seq || m === alias;
let followedByAnilist = new Map();
// Series followed from a MangaUpdates card (no AniList id), by title.
let followedByTitle = new Map();
// The key of the series this card is, when it's followed.
const followedOf = (c) => (c.anilist ? followedByAnilist.get(c.anilist) : c.mu ? followedByTitle.get(String(c.title || "").toLowerCase()) : undefined);
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

const cardKey = (c) => (c.anilist ? `al:${c.anilist}` : c.olid ? `ol:${c.olid}` : c.mu ? `mu:${c.mu}` : "");

/* ---------- hover cards, like Movies' ----------
   Resting on a cover opens it like a door: the cover stays where it is, a little
   larger, and the details swing out beside it (the banner on top, a few lines,
   the buttons). Same look and timing as the films' cards (browse.js), so both
   modes feel the same. No cards by touch or on a TV. */
const PV_INFO_W = 356;
const PV_H = 418;
const PV_POSTER_W = Math.round(PV_H * 2 / 3);
let pv = null;
let pvTimer = 0;
let pvLeave = 0;

function pvSchedule(anchor, c, shelf) {
  if (matchMedia("(hover: none)").matches || document.documentElement.classList.contains("tv")) return;
  clearTimeout(pvTimer);
  clearTimeout(pvLeave);
  if (c.banner) { const im = new Image(); im.src = c.banner; }
  pvTimer = setTimeout(() => pvShow(anchor, c, shelf), pv ? 140 : 420);
}

function pvSoon() {
  clearTimeout(pvTimer);
  clearTimeout(pvLeave);
  pvLeave = setTimeout(() => pvHide(), 160);
}

function pvHide(now = false) {
  clearTimeout(pvTimer);
  if (!pv) return;
  const p = pv;
  pv = null;
  p.style.pointerEvents = "none";
  if (now || !p.animate) { p.remove(); return; }
  const out = p.animate([{ opacity: 1, transform: "none" }, { opacity: 0, transform: "scale(0.96)" }], { duration: 160, easing: "ease-in", fill: "forwards" });
  out.onfinish = out.oncancel = () => p.remove();
}

function pvOpen(c, shelf) {
  pvHide(true);
  if (shelf) openSeries({ key: shelf.key });
  else if (isBook(c)) openBookPanel(c);
  else openSeries({ card: c });
}

function pvCard(c, shelf, door) {
  const act = (fn) => (e) => { e.stopPropagation(); fn(); };
  const round = (label, path, onclick) => el("button", { class: "pv-btn", "aria-label": label, title: label, onclick: act(onclick) }, icon(path));
  const pill = (label, path, onclick) => el("button", { class: "pv-pill", onclick: act(onclick) }, icon(path), label);
  const key = shelf ? shelf.key : followedOf(c);
  const actions = el("div", { class: "pv-actions" });
  if (key && !isBook(c)) actions.append(pill(shelf && shelf.resume_label ? `Read ${shelf.resume_label}` : "Read", ICON.play, () => { pvHide(true); openSeries({ key, card: c }); }));
  else actions.append(pill(isBook(c) ? "Get it" : "See chapters", isBook(c) ? ICON.download : ICON.info, () => pvOpen(c, shelf)));
  const lk = cardKey(c);
  if (lk) {
    const b = round(listKeys.has(lk) ? "On My List" : "My List", listKeys.has(lk) ? ICON.check : ICON.plus, async () => {
      const on = !listKeys.has(lk);
      try {
        await invoke("read_list_set", { card: c, on });
        if (on) listKeys.add(lk); else listKeys.delete(lk);
        b.replaceChildren(icon(on ? ICON.check : ICON.plus));
        b.title = on ? "On My List" : "My List";
        toast(on ? `Added ${c.title} to My List.` : `Took ${c.title} off My List.`);
      } catch (err) { toast(String(err), true); }
    });
    actions.append(b);
  }
  actions.append(el("span", { style: "flex:1" }), round("More info", "m6 9 6 6 6-6", () => pvOpen(c, shelf)));
  const src = c.banner || c.cover;
  const pic = src ? el("img", { class: "pv-img", src, alt: "", decoding: "async" }) : el("div", { class: "pv-img noart" });
  if (src) {
    if (pic.complete && pic.naturalWidth) pic.classList.add("loaded");
    else pic.addEventListener("load", () => pic.classList.add("loaded"), { once: true });
  }
  const meta = [];
  if (c.score) meta.push(el("span", { class: "rating", text: `\u2605 ${(c.score / 10).toFixed(1)}` }));
  if (c.year) meta.push(el("span", { text: String(c.year) }));
  meta.push(el("span", { class: "kind", text: kindOf(c) }));
  if (key) meta.push(el("span", { class: "owned", text: shelf && shelf.fresh ? `${shelf.fresh} new` : "Following" }));
  else if (STATUS[c.status]) meta.push(el("span", { text: STATUS[c.status] }));
  if (c.chapters) meta.push(el("span", { text: `${c.chapters} ch` }));
  const desc = String(c.description || "").replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim();
  const info = el("div", { class: "pv-info" },
    el("div", { class: "pv-art" }, pic),
    el("div", { class: "pv-body" },
      el("strong", { class: "pv-title", text: c.title }),
      el("div", { class: "pv-meta" }, meta),
      desc ? el("p", { class: "pv-overview", text: desc }) : null,
      c.genres && c.genres.length ? el("div", { class: "pv-genres" }, c.genres.slice(0, 3).map((g) => el("span", { text: g }))) : null,
      actions));
  const poster = door ? el("div", { class: "pv-poster" }, el("img", { src: c.cover, alt: "", decoding: "async" })) : null;
  return el("div", { class: door ? "preview door read-pv" : "preview read-pv", role: "dialog", "aria-label": c.title }, ...[poster, info].filter(Boolean));
}

function pvShow(anchor, c, shelf) {
  const art = anchor.querySelector(".art");
  if (!art || !anchor.isConnected || !anchor.matches(":hover") || (dlg && dlg.open) || document.querySelector(".reader")) return;
  pvHide(true);
  const r = art.getBoundingClientRect();
  const door = !!c.cover && r.height > r.width && innerWidth >= PV_POSTER_W + PV_INFO_W + 40;
  const w = door ? PV_POSTER_W + PV_INFO_W : Math.min(Math.max(r.width * 1.5, 340), PV_INFO_W + 40);
  const h = door ? PV_H : w * 9 / 16 + (PV_H - PV_INFO_W * 9 / 16);
  const cx = r.left + r.width / 2;
  const flip = door && cx + PV_POSTER_W / 2 + PV_INFO_W > innerWidth - 12;
  let left = door ? (flip ? cx + PV_POSTER_W / 2 - w : cx - PV_POSTER_W / 2) : cx - w / 2;
  left = Math.min(Math.max(12, left), innerWidth - w - 12);
  const top = Math.min(Math.max(76, r.top + r.height / 2 - h / 2), innerHeight - h - 12);
  const card = pvCard(c, shelf, door);
  if (flip) card.classList.add("flip");
  card.style.cssText = `left:${left}px;top:${top}px;width:${w}px;height:${h}px;--pw:${PV_POSTER_W}px`;
  card.addEventListener("mouseenter", () => clearTimeout(pvLeave));
  card.addEventListener("mouseleave", () => pvSoon());
  card.addEventListener("click", () => pvOpen(c, shelf));
  document.body.append(card);
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (card.animate && !still) {
    if (door) {
      const px = flip ? w - PV_POSTER_W : 0;
      const s = Math.max(0.5, r.width / PV_POSTER_W);
      const dx = cx - (left + px + PV_POSTER_W / 2);
      const dy = r.top + r.height / 2 - (top + h / 2);
      card.style.transformOrigin = `${px + PV_POSTER_W / 2}px ${h / 2}px`;
      const shut = flip ? `inset(0 0 0 ${w - PV_POSTER_W}px round 10px)` : `inset(0 ${w - PV_POSTER_W}px 0 0 round 10px)`;
      card.animate([{ transform: `translate(${dx}px, ${dy}px) scale(${s})`, clipPath: shut }, { transform: "none", clipPath: "inset(0 0 0 0 round 10px)" }],
        { duration: 340, easing: "cubic-bezier(0.16, 1, 0.3, 1)" });
    } else {
      const dx = cx - (left + w / 2);
      const dy = r.top + r.height / 2 - (top + h / 2);
      card.animate([{ opacity: 0, transform: `translate(${dx}px, ${dy}px) scale(${Math.max(0.5, r.width / w)})` }, { opacity: 1, transform: "none" }],
        { duration: 300, easing: "cubic-bezier(0.16, 1, 0.3, 1)" });
    }
  }
  pv = card;
}

// A scroll, a tab change or a panel opening closes the card.
window.addEventListener("scroll", () => { if (pv) pvHide(); }, { passive: true, capture: true });
window.addEventListener("hashchange", () => pvHide(true));

/** A card for an AniList or Open Library title, or a series you follow (`shelf`). */
function card(c, shelf = null) {
  const box = cover(c.cover, c.title);
  box.append(el("span", { class: "badge", text: kindOf(c) }));
  // The translation it's read in, on the cover, like a season on a show.
  const lang = shelf ? shelf.lang : c.anilist && followedLang.get(c.anilist);
  if (lang) box.append(el("span", { class: "badge read-lang", text: lang.toUpperCase() }));
  if (shelf && shelf.fresh) box.append(el("span", { class: "badge get read-new fresh-badge", text: shelf.fresh === 1 ? "NEW" : `${shelf.fresh} NEW` }));
  const sub = shelf ? shelf.resume_label || (shelf.finished ? "Finished" : shelf.downloaded ? "All read" : "Downloading") : c.author || (c.year ? String(c.year) : "");
  // What the Midnight and Classic styles add (styles.css shows each only in its style).
  if (c.score) box.append(el("span", { class: "rc-score", text: `\u2605 ${(c.score / 10).toFixed(1)}` }));
  const chs = shelf ? shelf.chapters : c.chapters;
  const extra = el("div", { class: "rc-extra" },
    ...[c.genres && c.genres.length ? el("span", { class: "rc-genre", text: c.genres[0] }) : null,
      chs ? el("span", { class: "rc-ch", text: `${chs} chapters` }) : null].filter(Boolean));
  const b = el("button", { class: "tile", "aria-label": c.title }, box,
    el("div", { class: "cap" }, el("strong", { text: c.title }), sub ? ` · ${sub}` : ""), extra);
  b.addEventListener("click", () => pvOpen(c, shelf));
  b.addEventListener("mouseenter", () => pvSchedule(b, c, shelf));
  b.addEventListener("mouseleave", () => pvSoon());
  if (styleNow() === "classic" && c.kind !== "book" && c.kind !== "comic") classicCaption(b, c, shelf);
  if (shelf) b._sel = shelf; // a series you follow can be picked with others (select.js)
  return markHidden(b, c, shelf);
}

/** Classic's caption: the title, then one quiet line (genre in colour, then length or where you are). */
function classicCaption(t, c, shelf) {
  const cap = t.querySelector(".cap");
  const second = shelf ? shelf.resume_label || "" : (c.chapters ? `${c.chapters} ch` : c.year ? String(c.year) : "");
  if (cap) cap.replaceWith(el("div", { class: "cap cl-cap" }, el("strong", { text: c.title })));
  const line = el("div", { class: "rc-extra cl-line" },
    ...[c.genres && c.genres.length ? el("span", { class: "rc-genre", text: c.genres[0] }) : null,
      second ? el("span", { class: "rc-ch", text: second }) : null].filter(Boolean));
  const extra = t.querySelector(".rc-extra");
  if (extra) extra.replaceWith(line); else t.append(line);
}

/** A Top 10 card: the big outlined number beside the cover, like the films' Top 10. */
function topTen(c, rank) {
  const t = card(c);
  t.classList.add("top10");
  t.prepend(el("span", { class: "rank", "aria-hidden": "true", text: String(rank) }));
  t.querySelector(".cap")?.remove();
  t.querySelector(".rc-extra")?.remove();
  return t;
}

/** A card for a book on your list. */
function bookCard(x) {
  const box = cover(x.cover, x.title);
  box.append(el("span", { class: "badge", text: x.comic ? "Comic" : "Book" }));
  const done = x.finished || x.read >= 1;
  if (x.read > 0 && !done) box.append(el("div", { class: "bar" }, el("span", { style: `width:${Math.round(x.read * 100)}%` })));
  const sub = x.problem ? "Needs a look" : !x.ready ? (x.progress != null ? `Downloading ${Math.round(x.progress * 100)}%` : "Starting") : done ? "Finished" : x.read > 0 ? `${Math.round(x.read * 100)}% read` : x.author || "";
  const b = el("button", { class: "tile", "aria-label": x.title }, box, el("div", { class: "cap" }, el("strong", { text: x.title }), sub ? ` · ${sub}` : ""));
  b.addEventListener("click", () => openBookPanel({ kind: x.comic ? "comic" : "book", title: x.title, author: x.author, cover: x.cover, description: x.description, olid: x.olid, anilist: x.anilist }, x));
  b._sel = x;
  return markHidden(b, x);
}

/* ---------- several at once (select.js) ---------- */
const plural = (n, one, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Deletes the downloaded chapters of every series picked; they stay followed. */
async function deleteSeriesDownloads(list) {
  const plans = await Promise.all(list.map((s) => invoke("read_delete_plan", { key: s.key, ids: null }).then((p) => ({ s, p })).catch(() => null)));
  const real = plans.filter((x) => x && x.p.chapters);
  if (!real.length) { toast("None of those have chapters downloaded."); return false; }
  const n = real.reduce((a, x) => a + x.p.chapters, 0);
  const bytes = real.reduce((a, x) => a + (x.p.bytes || 0), 0);
  const others = [...new Set(real.flatMap((x) => x.p.others || []))];
  const body = others.length
    ? `${others.join(", ")} also read${others.length === 1 ? "s" : ""} some of these. The files go for them too. You keep following them and can download the chapters again any time.`
    : "You keep following them. The chapters can be downloaded again any time.";
  if (!(await ask(`Delete ${plural(n, "chapter")} of ${plural(real.length, "series", "series")} (${bytesText(bytes)})?`, body, real.map((x) => x.s.title), "Delete", "Cancel"))) return false;
  let ok = 0;
  for (const x of real) {
    try { await invoke("read_delete_chapters", { key: x.s.key, ids: null, othersOk: true }); ok++; } catch (e) { toast(`${x.s.title}: ${e}`, true); }
  }
  if (ok) toast(`Deleted the downloads of ${plural(ok, "series", "series")}.`);
  window.dispatchEvent(new Event("chillarr:refresh"));
  return ok > 0;
}

/** Stops following every series picked, after asking whether their chapters go too. */
async function unfollowMany(list) {
  if (!(await ask(`Stop following ${plural(list.length, "series", "series")}?`, "New chapters stop downloading for them.", list.map((s) => s.title), "Stop following", "Cancel"))) return false;
  // Closing this question (Esc) stops nothing.
  const pick = await choose("Delete their chapters too?", "Keep them to read later, or free the space now.", [], "Delete them", "Keep them");
  if (pick == null) return false;
  const del = pick === "yes";
  toast(`Stopping ${plural(list.length, "series", "series")}...`);
  let ok = 0;
  for (const s of list) {
    try { await invoke("read_unfollow", { key: s.key, delete: del }); ok++; followedByAnilist.delete(s.anilist); } catch (e) { toast(`${s.title}: ${e}`, true); }
  }
  if (ok) toast(`Stopped following ${plural(ok, "series", "series")}.`);
  window.dispatchEvent(new Event("chillarr:refresh"));
  return ok > 0;
}

/** Deletes the files of every book picked; they stay in the list as not downloaded. */
async function deleteBookFiles(list) {
  const ready = list.filter((x) => x.ready && !x.not_downloaded);
  if (!ready.length) { toast("None of those have a file here yet."); return false; }
  const bytes = ready.reduce((a, x) => a + (x.bytes || 0), 0);
  if (!(await ask(`Delete the files of ${plural(ready.length, "book")}${bytes ? ` (${bytesText(bytes)})` : ""}?`,
    "They stay in your list as not downloaded. You can download them again any time.", ready.map((x) => x.title), "Delete files", "Cancel"))) return false;
  let ok = 0;
  const shared = [];
  for (const x of ready) {
    try { await invoke("read_book_delete_file", { key: x.key, othersOk: false }); ok++; }
    catch (e) { if (/still reads/.test(String(e))) shared.push(x); else toast(`${x.title}: ${e}`, true); }
  }
  if (shared.length && await ask(`Someone else still reads ${plural(shared.length, "of these", "of these")}`, "Their files go for them too. Delete them anyway?", shared.map((x) => x.title), "Delete files", "Keep those")) {
    for (const x of shared) {
      try { await invoke("read_book_delete_file", { key: x.key, othersOk: true }); ok++; } catch (e) { toast(`${x.title}: ${e}`, true); }
    }
  }
  if (ok) toast(`Deleted the files of ${plural(ok, "book")}.`);
  window.dispatchEvent(new Event("chillarr:refresh"));
  return ok > 0;
}

/** Takes every book picked off your list, its file too. */
async function removeBooks(list) {
  if (!(await ask(`Remove ${plural(list.length, "book")}?`, "Their files are deleted from your PC. You can download them again.", list.map((x) => x.title), "Remove", "Cancel"))) return false;
  let ok = 0;
  for (const x of list) {
    try { await invoke("read_book_remove", { key: x.key }); ok++; } catch (e) { toast(`${x.title}: ${e}`, true); }
  }
  if (ok) toast(`Removed ${plural(ok, "book")}.`);
  window.dispatchEvent(new Event("chillarr:refresh"));
  return ok > 0;
}

/* Who may delete what (read_storage::may_delete): the PC decides per series or
   book, on phones too. Asked once a panel opens, kept by key. */
const delOk = new Map();
async function mayDelete(key) {
  if (!delOk.has(key)) delOk.set(key, await invoke("read_can_delete", { key }).then((x) => !!x.can).catch(() => false));
  return delOk.get(key);
}

/** A Select button on a row of series you follow, or of your books (the PC refuses what isn't yours). */
function withSelect(section, kind) {
  if (!section) return section;
  const head = section.querySelector(".row-head");
  const cardsOf = () => [...section.querySelectorAll(".tile")].map((n) => [n, n._sel]);
  const actions = kind === "books"
    ? [{ label: "Delete files", icon: TRASH, run: deleteBookFiles }, { label: "Remove", danger: true, run: removeBooks }]
    : [{ label: "Delete downloads", icon: TRASH, run: deleteSeriesDownloads }, { label: "Stop following", danger: true, run: unfollowMany }];
  head.append(selectable(section, cardsOf, actions));
  return section;
}

/** Where a row's "Explore all" goes: every title in it, a page at a time (allPage). */
const allHref = (id, title) => `#r-all/${encodeURIComponent(id)}/${encodeURIComponent(title)}`;
const hasAll = (id) => id in ROW_EXPLORE || /^(books-|comics-|foryou-|because-\d|k\.)/.test(id);

/** A Manga/Manhwa/Manhua/Novels row ("k.<kind>.<row>") as the query for its whole page, like read.rs kind_row_query. */
function kindRowQuery(id) {
  const m = /^k\.(manga|manhwa|manhua|novels)\.(.+)$/.exec(id);
  if (!m) return null;
  const [, kind, r] = m;
  if (r === "trending") return { kind, sort: "trending" };
  if (r === "popular") return { kind, sort: "popular" };
  if (r === "top") return { kind, sort: "score" };
  if (r === "ongoing") return { kind, sort: "popular", status: "RELEASING" };
  if (r === "new") return { kind, sort: "popular", year_from: new Date().getFullYear() - 1 };
  if (r.startsWith("g:")) return { kind, sort: "popular", genres: [r.slice(2)] };
  return null;
}
/** Rows of your own that open a page of their own instead. */
const ROW_HOME = { reading: "#r-mylist", finished: "#r-mylist", books: "#r-books" };

/** A row whose covers all fit shows no arrows and doesn't scroll, like Movies'. */
const fitsRO = typeof ResizeObserver === "function" ? new ResizeObserver((entries) => {
  for (const e of entries) {
    const t = e.target;
    if (!t.isConnected) { fitsRO.unobserve(t); continue; }
    t.parentElement.classList.toggle("fits", t.scrollWidth - t.clientWidth < 48);
  }
}) : null;

function row(id, title, cards, allId = id) {
  const track = el("div", { class: "track" }, cards);
  const step = () => track.clientWidth * 0.85;
  // Like Movies: the title and "Explore all" open every title in the row.
  const all = hasAll(allId) ? allHref(allId, title === "Top 10 today" ? "Trending now" : title) : ROW_HOME[allId] || null;
  const more = all && !hasAll(allId) ? "See all" : "Explore all";
  const wrap = el("div", { class: "track-wrap" },
    el("button", { class: "arrow prev", "aria-label": "Scroll left", onclick: () => track.scrollBy({ left: -step(), behavior: "smooth" }) }, icon(ICON.left)),
    track,
    el("button", { class: "arrow next", "aria-label": "Scroll right", onclick: () => track.scrollBy({ left: step(), behavior: "smooth" }) }, icon(ICON.right)));
  fitsRO?.observe(track);
  track.addEventListener("scroll", () => { if (pv) pvHide(true); }, { passive: true });
  return el("section", { class: "row", dataset: { row: id } },
    el("div", { class: "row-head" }, all ? el("a", { class: "row-title", href: all }, el("h2", { text: title })) : el("h2", { text: title }),
      all ? el("a", { class: "explore", href: all }, more, icon(ICON.right)) : el("span", { class: "count", text: String(cards.length) })),
    wrap);
}

/** read.rs rows() as the same query for a whole page (read_explore). */
const ROW_EXPLORE = {
  trending: { kind: "all", sort: "trending" },
  manhwa: { kind: "manhwa", sort: "popular" },
  manhua: { kind: "manhua", sort: "popular" },
  manga: { kind: "manga", sort: "popular" },
  top: { kind: "all", sort: "score" },
  new: { kind: "all", status: "RELEASING", year_from: 2024, sort: "popular" },
  action: { kind: "all", genres: ["Action"], sort: "popular" },
  romance: { kind: "all", genres: ["Romance"], sort: "popular" },
};

const asCard = (s) => ({ anilist: s.anilist, title: s.title, cover: s.cover, country: s.country, status: s.status });
const page = (...kids) => el("div", { class: "read-page" }, ...kids);
const head = (title, ...rest) => el("header", { class: "read-head" }, el("h1", { text: title }), ...rest);
const skel = () => [el("div", { class: "skel skel-row" }), el("div", { class: "skel skel-row" })];
const note = (text) => el("p", { class: "note", text });

async function know() {
  const [lib, list] = await Promise.all([invoke("read_library").catch(() => []), invoke("read_list").catch(() => [])]);
  followedByAnilist = new Map(lib.filter((s) => s.anilist).map((s) => [s.anilist, s.key]));
  followedByTitle = new Map(lib.filter((s) => !s.anilist).map((s) => [String(s.title || "").toLowerCase(), s.key]));
  followedLang = new Map(lib.filter((s) => s.anilist && s.lang).map((s) => [s.anilist, s.lang]));
  listKeys = new Set(list.map(cardKey));
  freshBadge(lib);
  return { lib, list };
}

/** The reading Home tab's bubble: how many chapters are new, across everything you follow. */
function freshBadge(lib) {
  const n = lib.reduce((sum, s) => sum + (s.fresh || 0), 0);
  for (const a of document.querySelectorAll('a[data-tab="r-home"]')) {
    let b = a.querySelector(".rt-badge");
    if (!n) { if (b) b.remove(); continue; }
    if (!b) { b = el("span", { class: "rt-badge" }); a.append(b); }
    if (b.textContent !== String(n)) { b.textContent = n > 99 ? "99+" : String(n); b.classList.remove("pop"); void b.offsetWidth; b.classList.add("pop"); }
  }
  // Once a visit (per profile): say which series have new chapters.
  const fresh = lib.filter((s) => s.fresh).sort((a, b) => b.fresh_at - a.fresh_at);
  const sig = fresh.map((s) => `${s.key}:${s.fresh}`).join(",");
  let told = "";
  try { told = sessionStorage.getItem(`read-fresh-told:${fresh.length ? "x" : ""}`) || ""; } catch (e) { /* storage blocked */ }
  if (fresh.length && told !== sig) {
    try { sessionStorage.setItem("read-fresh-told:x", sig); } catch (e) { /* storage blocked */ }
    const names = fresh.slice(0, 3).map((s) => `${s.title} (${s.fresh})`).join(", ");
    toast(`New chapters: ${names}${fresh.length > 3 ? ` and ${fresh.length - 3} more` : ""}.`);
  }
}

/** "New chapters" at the top of Home: each series with chapters out since you last read it. */
function freshCards(lib) {
  return lib.filter((s) => s.fresh).sort((a, b) => b.fresh_at - a.fresh_at).map((s, i) => {
    const t = card(asCard(s), s);
    t.classList.add("fresh-tile", "rise");
    t.style.setProperty("--i", String(i));
    const cap = t.querySelector(".cap");
    if (cap) cap.replaceChildren(el("strong", { text: s.title }), el("span", { class: "fresh-ch", text: s.fresh_label || "New" }), el("small", { class: "fresh-when", text: ago(s.fresh_at * 1000) }));
    t.querySelector(".rc-extra")?.remove();
    return t;
  });
}

function freshSection(lib, cls = "") {
  const cards = freshCards(lib);
  if (!cards.length) return null;
  const n = lib.reduce((sum, s) => sum + (s.fresh || 0), 0);
  return el("section", { class: `fresh-block ${cls}` },
    el("div", { class: "fresh-head" }, el("span", { class: "fresh-dot", "aria-hidden": "true" }), el("h2", { text: "New chapters" }),
      el("span", { class: "fresh-count", text: `${n} new` })),
    el("div", { class: "fresh-track" }, cards));
}

/* ---------- the tabs ---------- */

/** Draws one reading tab: r-home, r-manga, r-manhwa, r-manhua, r-novels, r-books, r-mylist, r-downloads. */
/* ---------- Tabs kept between visits ----------
   Like the watching tabs (browse.js): a reading tab's finished page is kept
   when you leave it and put straight back when you return, instead of asking
   AniList and Open Library for it all again (Books took seconds every time).
   Home and Books show your progress and new chapters, so they're kept for a
   few minutes, and anything that changes what you read (chillarr:refresh)
   lets them go. The tabs' grids (Manga, Manhwa...) come back where they were
   and keep loading as you scroll. My List and Downloads are drawn again:
   they're quick, and always current. */
const KEEP_MS = { "r-home": 3 * 60000, "r-books": 3 * 60000 };
// Manga, Manhwa, Manhua, Novels: AniList's rankings, kept longer. Their grid
// keeps loading as you scroll after it comes back (explorePage's resume).
const KEEP_GRID_MS = 15 * 60000;
const kept = new Map(); // `${person}:${style}:${tab}` -> { nodes, at, tab }
let shown = null; // the reading page on screen: { key, tab, view, done }
const keepFor = (tab) => KEEP_MS[tab] || (TAB_TITLE[tab] ? KEEP_GRID_MS : 0);

/** The page on screen, kept if it finished drawing. */
function stash() {
  const s = shown;
  shown = null;
  // A page that didn't get its rows (a source wasn't answering) isn't kept.
  if (!s || !s.done || !keepFor(s.tab) || !s.view.querySelector(".track, .read-grid > *")) return;
  kept.set(s.key, { nodes: [...s.view.childNodes], at: Date.now(), tab: s.tab, my: s.my });
  while (kept.size > 8) kept.delete(kept.keys().next().value);
}
// Something you read changed: Home and Books go (they show it); the grids stay.
window.addEventListener("chillarr:refresh", () => { for (const [k, e] of kept) if (!TAB_TITLE[e.tab]) kept.delete(k); });

export async function renderRead(view, tab) {
  stash();
  alias = -1;
  const my = ++seq;
  stopSelecting();
  currentView = view;
  currentTab = tab;
  applyLook();
  pvHide(true);
  drawPickers(); // the top bar's Style picker, on every reading tab
  // The style is part of the key: a page kept in another style is never put
  // back after a style change (it used to be, so only the colours changed).
  const key = `${personKey()}:${styleNow()}:${tab}`;
  const back = kept.get(key);
  kept.delete(key);
  if (back && Date.now() - back.at < keepFor(tab)) {
    alias = back.my;
    view.replaceChildren(...back.nodes);
    // Its banner turns again and its grid watches for the bottom again.
    for (const n of view.children) n.resume?.();
    view.querySelectorAll(".hero").forEach((h) => h.resume?.());
    shown = { key, tab, view, done: true, my: back.my };
    return;
  }
  const me = { key, tab, view, done: false, my };
  shown = me;
  const drawn = TAB_TITLE[tab] ? explorePage(view, tab, my)
    : tab === "r-all" ? allPage(view, my)
    : tab === "r-author" ? authorPage(view, my)
    : tab === "r-maker" ? makerPage(view, my)
    : tab === "r-books" ? booksPage(view, my)
    : tab === "r-mylist" ? listPage(view, my)
    : tab === "r-downloads" ? downloadsPage(view, my)
    : homePage(view, my);
  // Kept only once it's all in: a page left half drawn stops (its seq moved on).
  Promise.resolve(drawn).then(() => { if (alive(my)) me.done = true; }, () => {});
  return drawn;
}

export const stopRead = () => { stash(); alias = -1; seq++; };

/** Reading tabs that are one long grid from AniList or Open Library: what's
 *  on them doesn't change when something happens elsewhere (a chapter read,
 *  a download landing), and drawing them again lost your place far down. */
const GRID_TABS = new Set(["r-manga", "r-manhwa", "r-manhua", "r-novels", "r-all", "r-author", "r-maker"]);

/** The app-wide refresh on a reading tab. True when it was handled in place (no redraw). */
export function softRefresh(tab) {
  if (!GRID_TABS.has(tab) || currentTab !== tab) return false;
  know().catch(() => {});
  return true;
}

/* ---------- the big picture at the top ---------- */

let heroTimer = 0;
/**
 * The featured series, like Movies' hero: the banner behind, softened, the cover
 * large on the right, the title, a few lines and the buttons; it moves on every
 * 9 seconds, or with the dots, arrows or a swipe. AniList banners are short
 * strips, so the cover carries the picture.
 */
function readHero(picks) {
  clearInterval(heroTimer);
  if (!picks.length) return null;
  let at = 0;
  const bg = (c) => (c.banner || c.cover ? `url("${String(c.banner || c.cover).replace(/"/g, "%22")}")` : "none");
  // A banner is a wide strip and only needs softening; a cover alone is shown as it is (no blur).
  const kindBg = (n, c) => n.classList.toggle("is-cover", !c.banner);
  const bgA = el("div", { class: "hero-bg read-hero-bg", style: `background-image:${bg(picks[0])}` });
  kindBg(bgA, picks[0]);
  const bgB = el("div", { class: "hero-bg read-hero-bg out" });
  const art = el("div", { class: "read-hero-art" });
  const section = el("section", { class: "hero read-hero-top" }, bgA, bgB, copy(picks[0]), art);
  section.stale = () => picks.some((c) => isHidden(c));
  section.rebuild = () => readHero(picks.filter((c) => !isHidden(c))) || el("section", { class: "hero empty" });
  drawArt(picks[0]);
  let front = bgA, back = bgB;
  section.addEventListener("click", (e) => { if (!e.target.closest("button, a")) open(picks[at]); });
  function open(c) { const key = followedByAnilist.get(c.anilist); openSeries(key ? { key, card: c } : { card: c }); }
  function copy(c) {
    const key = c.anilist && followedByAnilist.get(c.anilist);
    const desc = String(c.description || "").replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim();
    return el("div", { class: "hero-copy" },
      el("p", { class: "hero-kicker", text: key ? "Continue reading" : `Popular ${kindOf(c).toLowerCase()}` }),
      el("h1", { text: c.title }),
      el("p", { class: "meta" }, ...[kindOf(c), STATUS[c.status], c.chapters ? `${c.chapters} chapters` : null, c.score ? `${c.score}%` : null, ...(c.genres || []).slice(0, 2)]
        .filter(Boolean).flatMap((x, i) => (i ? [el("span", { class: "sep" }), el("span", { text: x })] : [el("span", { text: x })]))),
      desc ? el("p", { class: "overview", text: desc }) : null,
      el("div", { class: "actions" },
        el("button", { class: "btn play", onclick: () => open(c) }, icon(key ? ICON.play : ICON.info), key ? "Continue reading" : "See chapters"),
        listButton(c)));
  }
  function drawArt(c) {
    const img = c.cover ? el("img", { src: c.cover, alt: "", decoding: "async" }) : null;
    if (img) img.addEventListener("load", () => img.classList.add("loaded"), { once: true });
    art.replaceChildren(...(img ? [img] : []));
  }
  if (picks.length < 2) return section;
  const dots = el("div", { class: "hero-dots" }, picks.map((_, i) => el("button", { class: i === 0 ? "on" : "", "aria-label": `Show featured series ${i + 1}`, onclick: () => show(i) })));
  // The arrows sit with the dots, bottom right, clear of the words.
  const side = (cls, label, path, step) => el("button", { class: `hero-arrow ${cls}`, "aria-label": label, onclick: () => show(at + step) }, icon(path));
  section.append(el("div", { class: "hero-nav" }, side("prev", "Previous series", ICON.left, -1), dots, side("next", "Next series", ICON.right, 1)));
  let downX = null;
  section.addEventListener("pointerdown", (e) => { if (!e.target.closest("button, a")) downX = e.clientX; });
  section.addEventListener("pointerup", (e) => {
    if (downX == null) return;
    const dx = e.clientX - downX;
    downX = null;
    if (Math.abs(dx) > 60) show(at + (dx < 0 ? 1 : -1));
  });
  function show(i) {
    at = (i + picks.length) % picks.length;
    back.style.backgroundImage = bg(picks[at]);
    kindBg(back, picks[at]);
    back.classList.remove("out");
    front.classList.add("out");
    [front, back] = [back, front];
    section.querySelector(".hero-copy").replaceWith(copy(picks[at]));
    drawArt(picks[at]);
    [...dots.children].forEach((d, j) => d.classList.toggle("on", j === at));
    restart();
  }
  function restart() {
    clearInterval(heroTimer);
    heroTimer = setInterval(() => {
      if (!document.body.contains(section)) { clearInterval(heroTimer); return; }
      if (!document.hidden && !(dlg && dlg.open) && !document.querySelector(".reader")) show(at + 1);
    }, 9000);
  }
  restart();
  section.resume = restart;
  return section;
}

/** Up to six to feature: ones with a picture, the series you're reading first. */
function heroPicks(cards) {
  const seen = new Set();
  const out = [];
  for (const c of cards) {
    if (!c || !c.anilist || seen.has(c.anilist) || !(c.banner || c.cover) || !c.description || isHidden(c)) continue;
    seen.add(c.anilist);
    out.push(c);
  }
  const mine = out.filter((c) => followedByAnilist.has(c.anilist));
  return [...mine, ...out.filter((c) => !followedByAnilist.has(c.anilist))].slice(0, 6);
}

/* ---------- styles: how reading looks (look.js) ----------
   Cinema: big pictures and rows, like the films. Midnight: dark and dense, the
   newest chapters of what you follow up front, a ranking down the side.
   Classic: light, a numbered top 10, your series by the day they update, and
   popular by genre. */
let currentView = null;
let currentTab = "r-home";
// A new style draws the reading tab on screen again.
onStyle("read", () => (currentView && document.body.contains(currentView) ? renderRead(currentView, currentTab) : null));

/** "2 hours ago" for an upload time (Suwayomi gives milliseconds since 1970, as text). */
function ago(ms) {
  const t = Number(ms);
  if (!t) return "";
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 90) return "Just now";
  const m = s / 60, h = m / 60, d = h / 24;
  if (m < 60) return `${Math.round(m)} min ago`;
  if (h < 24) return `${Math.round(h)} hour${Math.round(h) === 1 ? "" : "s"} ago`;
  if (d < 7) return `${Math.round(d)} day${Math.round(d) === 1 ? "" : "s"} ago`;
  if (d < 30) return `${Math.round(d / 7)} week${Math.round(d / 7) === 1 ? "" : "s"} ago`;
  return new Date(t).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

/** Each followed series with its chapters (for updates and the week), newest first. */
async function followedWithChapters(series, most = 16) {
  // Hidden from the week by this profile: left out here (Your week, Latest updates).
  // The 16 that updated most lately: the list comes sorted by last read, so a
  // series you haven't opened in a while but that updated today still makes it.
  series = series.filter((s) => !s.hide_week).slice().sort((a, b) => (b.latest_at || 0) - (a.latest_at || 0));
  const got = await Promise.all(series.slice(0, most).map((s) => invoke("read_detail", { key: s.key }).then((d) => ({ s, d })).catch(() => null)));
  return got.filter(Boolean).map(({ s, d }) => {
    const chs = d.chapters.filter((c) => c.downloaded || c.uploaded).slice().sort((a, b) => (b.number ?? 0) - (a.number ?? 0));
    const newest = Math.max(0, ...d.chapters.map((c) => Number(c.uploaded) || 0));
    return { s, d, chs, newest, card: { ...asCard(s), banner: d.followed && d.followed.banner, description: d.followed && d.followed.description, genres: d.followed && d.followed.genres } };
  }).sort((a, b) => b.newest - a.newest);
}

/** A numbered ranking row (Midnight's side list, Classic's top 10). */
function rankRow(c, i) {
  const b = el("button", { class: "rk", style: `--i:${i}`, onclick: () => pvOpen(c, null) },
    el("span", { class: "rk-n", text: String(i + 1).padStart(2, "0") }),
    c.cover ? el("img", { class: "rk-img", src: c.cover, alt: "", loading: "lazy", decoding: "async" }) : el("span", { class: "rk-img" }),
    el("span", { class: "rk-text" },
      el("strong", { text: c.title }),
      el("small", { text: [(c.genres || []).slice(0, 2).join(", "), kindOf(c)].filter(Boolean).join(" \u00b7 ") })),
    c.score ? el("span", { class: "rk-score", text: `\u2605 ${(c.score / 10).toFixed(1)}` }) : null);
  return markHidden(b, c);
}

/** Tabs that swap a list: [[label, loader]], each loader returning cards. Watching's Your week uses it too. */
export function tabbed(cls, tabs, draw, start = 0) {
  const head = el("div", { class: "tabbed-tabs", role: "tablist" });
  const body = el("div", { class: "tabbed-body" });
  const cache = new Map();
  let want = 0;
  const pick = async (i, first = false) => {
    want = i;
    [...head.children].forEach((b, j) => { b.classList.toggle("on", j === i); b.setAttribute("aria-selected", String(j === i)); });
    // The old list fades out while the new one loads, then the new one fades in.
    const out = first ? null : new Promise((r) => setTimeout(r, 160));
    if (!first) body.classList.add("swap");
    let items = cache.get(i);
    if (!items) {
      // A failed load shows empty this time but is asked again next time.
      try { items = await tabs[i][1](); cache.set(i, items); } catch (e) { items = []; }
    }
    await out;
    if (want !== i) return; // another tab was picked meanwhile
    body.style.minHeight = first ? "" : `${body.offsetHeight}px`; // no jump while it swaps
    body.replaceChildren(...draw(items));
    void body.offsetWidth;
    body.classList.remove("swap");
    setTimeout(() => { body.style.minHeight = ""; }, 260);
  };
  head.append(...tabs.map(([label], i) => el("button", { role: "tab", onclick: () => pick(i) }, label)));
  pick(Math.min(Math.max(0, start), tabs.length - 1), true);
  return el("div", { class: `tabbed ${cls}` }, head, body);
}

/** Short-lived answers, so going back and forth between tabs doesn't ask AniList again each time. */
const recent = new Map();
function remembered(key, life, ask) {
  const hit = recent.get(key);
  if (hit && performance.now() - hit.at < life) return hit.p;
  const p = ask().catch((e) => { recent.delete(key); throw e; });
  recent.set(key, { at: performance.now(), p });
  if (recent.size > 60) recent.delete(recent.keys().next().value);
  return p;
}
const exploreItems = (e) => remembered(`x:${JSON.stringify(e)}`, 10 * 60000, () => invoke("read_explore", { e: { page: 1, ...e } })).then((g) => g.items || []);
/** Rows picked from what this profile reads (read.rs for_you); none on any failure. */
const forYou = () => remembered(`fy:${personKey()}`, 5 * 60000, () => invoke("read_for_you")).catch(() => []);

async function midnightHome(view, my) {
  const top = el("div", { class: "read-hero-slot" });
  const updates = el("section", { class: "mn-block" });
  const popular = el("section", { class: "mn-block" });
  const rows = el("div");
  const side = el("aside", { class: "mn-side" });
  view.replaceChildren(top, el("div", { class: "read-page mn-home" }, inlinePicker(), el("div", { class: "mn-cols" }, el("div", { class: "mn-main" }, updates, popular, rows), side)));
  drawPickers();
  const [{ lib: series }, got] = await Promise.all([know(), invoke("read_rows").catch(() => [])]);
  if (!alive(my)) return;
  const h = readHero(heroPicks(got.flatMap((r) => r.items)));
  if (h) { h.classList.add("compact"); top.replaceWith(h); }
  const trending = (got.find((r) => r.id === "trending") || got[0] || { items: [] }).items;
  side.append(el("h2", { class: "mn-h", text: "Popular" }), tabbed("mn-rank", [
    ["Trending", async () => trending.slice(0, 10)],
    ["Top rated", () => exploreItems({ kind: "all", sort: "score" }).then((x) => x.slice(0, 10))],
    ["Most read", () => exploreItems({ kind: "all", sort: "popular" }).then((x) => x.slice(0, 10))],
  ], (items) => items.map(rankRow)));
  popular.append(el("h2", { class: "mn-h", text: "Popular now" }), el("div", { class: "grid read-grid mn-grid" }, trending.slice(0, 18).map((c, i) => { const t = card(c); t.style.setProperty("--i", String(i)); t.classList.add("rise"); return t; })));
  rows.append(...got.filter((r) => r.items.length && r !== got.find((x) => x.id === "trending")).map((r) => row(r.id, r.title, r.items.map((c) => card(c)))));
  forYou().then((fy) => { if (alive(my) && fy.length) rows.prepend(...fy.map((r) => row(r.id, r.title, r.items.map((c) => card(c))))); });
  const freshMn = freshSection(series, "mn-fresh");
  if (freshMn) updates.before(freshMn);
  if (series.length) {
    updates.append(el("h2", { class: "mn-h", text: "Latest updates" }), el("div", { class: "skel skel-row" }));
    // Two rows at most, newest first: a newer chapter pushes the oldest out.
    const fit = () => Math.max(1, Math.floor(((updates.clientWidth || innerWidth) + 12) / (320 + 12))) * 2;
    const list = (await followedWithChapters(series.filter((s) => !isHidden(asCard(s), s)), fit() + 2)).filter((x) => !isHidden(x.card, x.s));
    if (!alive(my)) return;
    const grid = el("div", { class: "mn-updates" }, list.map((x, i) => {
      const n = markHidden(el("div", { class: "mn-up" + (lastUpdates.size && !lastUpdates.has(x.s.key) ? " mn-new" : ""), style: `--i:${i}` },
        el("button", { class: "mn-up-cover", "aria-label": x.s.title, onclick: () => openSeries({ key: x.s.key, card: x.card }) }, x.s.cover ? el("img", { src: x.s.cover, alt: "", loading: "lazy" }) : null),
        el("div", { class: "mn-up-text" },
          el("button", { class: "mn-up-title", onclick: () => openSeries({ key: x.s.key, card: x.card }), text: x.s.title }),
          ...x.chs.slice(0, 3).map((c) => el("button", { class: "mn-ch" + (c.read ? " read" : ""), disabled: !c.downloaded, onclick: () => read(x.s.key, c.id, x.card) },
            el("span", {}, chLabel(c), c.fresh ? el("b", { class: "read-new-pill", text: "NEW" }) : ""), el("small", { text: c.downloaded ? ago(c.uploaded) : "Not here yet" }))))), x.s, x.card);
      n.dataset.key = x.s.key;
      return n;
    }));
    const cap = () => { const n = fit(); [...grid.children].forEach((c, i) => { c.hidden = i >= n; }); };
    cap();
    lastUpdates = new Set([...grid.children].filter((c) => !c.hidden).map((c) => c.dataset.key));
    if (window.ResizeObserver) new ResizeObserver(cap).observe(updates);
    updates.replaceChildren(el("h2", { class: "mn-h", text: "Latest updates" }), grid);
  }
  return true;
}
// What Latest updates showed last, so a series newly in it slides in.
let lastUpdates = new Set();

const WEEK = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** Hides a series from Your week and Latest updates on this profile, or shows it again. */
async function setHideWeek(key, title, hide) {
  try {
    await invoke("read_hide_week", { key, hide });
    toast(hide ? `${title} is hidden from Your week. Show it again from its page.` : `${title} shows in Your week again.`);
    window.dispatchEvent(new Event("chillarr:refresh"));
    return true;
  } catch (e) { toast(String(e), true); return false; }
}

/** A Your week card with a small Hide button on it. */
function weekCard(t, s) {
  const b = el("button", { class: "week-hide", title: "Hide from Your week", "aria-label": `Hide ${s.title} from Your week`,
    onclick: async (e) => { e.stopPropagation(); e.preventDefault(); if (await setHideWeek(s.key, s.title, true)) t.remove(); } },
    icon("M3 3l18 18M10.6 6.1A10 10 0 0 1 12 6c5 0 9 6 9 6a17 17 0 0 1-3 3.5M6.6 6.6C4.3 8.1 3 12 3 12s4 6 9 6a9 9 0 0 0 4.4-1.1M9.9 9.9a3 3 0 0 0 4.2 4.2"));
  t.append(b);
  return t;
}
const CLASSIC_GENRES = ["Action", "Fantasy", "Romance", "Drama", "Comedy", "Thriller", "Slice of Life", "Supernatural"];

/** A Classic card: big cover, the genre in colour, the title, and an UP badge when it updated today. */
function classicCard(c, i, opts = {}) {
  const t = card(c, opts.shelf || null);
  t.classList.add("rise", "cl-card");
  t.style.setProperty("--i", String(i));
  if (opts.up) t.querySelector(".art").append(el("span", { class: "cl-up", text: "UP" }));
  return t;
}

/** Classic's ranking: a big number, a small cover, the title and genre, in columns like a chart. */
function classicRank(c, i) {
  const img = c.cover ? el("img", { src: c.cover, alt: "", loading: "lazy", decoding: "async" }) : null;
  if (img) img.addEventListener("load", () => img.classList.add("loaded"), { once: true });
  const b = el("button", { class: "cl-rk", style: `--i:${i}`, "aria-label": c.title },
    el("span", { class: "cl-rk-n", text: String(i + 1) }),
    el("span", { class: "cl-rk-art art" }, img),
    el("span", { class: "cl-rk-text" },
      el("strong", { text: c.title }),
      el("small", { text: [(c.genres || [])[0], kindOf(c)].filter(Boolean).join(" \u00b7 ") })));
  b.addEventListener("click", () => pvOpen(c, null));
  b.addEventListener("mouseenter", () => pvSchedule(b, c, null));
  b.addEventListener("mouseleave", () => pvSoon());
  return markHidden(b, c);
}

async function classicHome(view, my) {
  const ranks = el("section", { class: "cl-block" });
  const week = el("section", { class: "cl-block" });
  const byGenre = el("section", { class: "cl-block" });
  const fresh = el("section", { class: "cl-block" });
  view.replaceChildren(el("div", { class: "read-page cl-home" }, inlinePicker(), el("div", { class: "cl-banner" }, el("h1", { text: "Reading" }), el("p", { text: "What everyone's reading, and what's new in yours." })), ranks, week, byGenre, fresh));
  drawPickers();
  const [{ lib: series }, got] = await Promise.all([know(), invoke("read_rows").catch(() => [])]);
  if (!alive(my)) return;
  const freshCl = freshSection(series, "cl-block cl-fresh");
  if (freshCl) ranks.before(freshCl);
  const trending = (got.find((r) => r.id === "trending") || got[0] || { items: [] }).items;
  ranks.append(el("h2", { class: "cl-h", text: "Trending and popular" }), tabbed("cl-top", [
    ["Trending", async () => trending.slice(0, 10)],
    ["Popular", () => exploreItems({ kind: "all", sort: "popular" }).then((x) => x.slice(0, 10))],
    ["Top rated", () => exploreItems({ kind: "all", sort: "score" }).then((x) => x.slice(0, 10))],
  ], (items) => [el("div", { class: "cl-ranks" }, items.map(classicRank))]));
  // Picked for you: one tab per row made from what you read.
  forYou().then((fy) => {
    if (!alive(my) || !fy.length) return;
    const short = (t) => t.replace(/^Because you read /, "Like ").replace(/^More (.+) for you$/, "$1");
    byGenre.before(el("section", { class: "cl-block" }, el("h2", { class: "cl-h", text: "Picked for you" }),
      el("p", { class: "cl-sub", text: "From what you read." }),
      tabbed("cl-foryou", fy.map((r) => [short(r.title), async () => r.items.slice(0, 10)]),
        (items) => [el("div", { class: "cl-grid" }, items.map((c, i) => classicCard(c, i)))])));
  });
  byGenre.append(el("h2", { class: "cl-h", text: "Popular by genre" }), tabbed("cl-genres",
    CLASSIC_GENRES.map((g) => [g, () => exploreItems({ kind: "all", genres: [g], sort: "popular" }).then((x) => x.slice(0, 10))]),
    (items) => [el("div", { class: "cl-grid" }, items.map((c, i) => classicCard(c, i)))]));
  fresh.append(el("h2", { class: "cl-h", text: "New and ongoing" }), el("div", { class: "skel skel-row" }));
  exploreItems({ kind: "all", status: "RELEASING", sort: "new" }).then((x) => {
    if (!alive(my)) return;
    fresh.replaceChildren(el("h2", { class: "cl-h", text: "New and ongoing" }), el("div", { class: "cl-grid" }, x.filter((c) => c.cover).slice(0, 10).map((c, i) => classicCard(c, i))));
  }).catch(() => fresh.remove());
  if (series.length) {
    const list = await followedWithChapters(series);
    if (!alive(my)) return;
    const today = new Date().getDay();
    const day = (x) => (x.newest ? new Date(x.newest).getDay() : -1);
    const tabs = WEEK.map((d, i) => [i === today ? `${d} (today)` : d, async () => list.filter((x) => day(x) === i)]);
    // Today first, then the days after it.
    const order = [...WEEK.keys()].map((k) => (today + k) % 7);
    week.append(el("h2", { class: "cl-h", text: "Your week" }), el("p", { class: "cl-sub", text: "Your series by the day their chapters usually come out." }),
      tabbed("cl-week", order.map((i) => tabs[i]), (items) => (items.length
        ? [el("div", { class: "cl-grid" }, items.map((x, i) => weekCard(classicCard(x.card, i, { shelf: x.s, up: Date.now() - x.newest < 86400000 }), x.s)))]
        : [el("p", { class: "cl-empty", text: "Nothing of yours comes out that day." })])));
  }
  return true;
}

async function homePage(view, my) {
  const style = styleNow();
  applyLook();
  if (style === "midnight") return midnightHome(view, my);
  if (style === "classic") return classicHome(view, my);
  drawPickers();
  const lib = el("div");
  const rows = el("div", {}, ...skel());
  const top = el("div", { class: "read-hero-slot" });
  view.replaceChildren(top, page(inlinePicker(), head("Reading"), lib, rows));
  const [{ lib: series }, books] = await Promise.all([know(), invoke("read_books").catch(() => [])]);
  if (!alive(my)) return;
  myBooks = books;
  const reading = series.filter((s) => !s.finished);
  const done = series.filter((s) => s.finished);
  const out = [];
  const fresh = freshSection(series);
  if (fresh) out.push(fresh);
  if (reading.length) out.push(withSelect(row("reading", "Continue reading", reading.map((s) => card(asCard(s), s))), "series"));
  const unread = (b) => !b.finished && b.read < 1;
  if (books.filter((b) => b.ready && unread(b)).length) out.push(withSelect(row("books", "Your books", books.filter(unread).map(bookCard)), "books"));
  if (done.length) out.push(withSelect(row("finished", "Finished", done.map((s) => card(asCard(s), s))), "series"));
  lib.replaceChildren(...out);
  try {
    const got = await invoke("read_rows");
    if (!alive(my)) return;
    const h = readHero(heroPicks(got.flatMap((r) => r.items)));
    if (h) { top.replaceWith(h); view.querySelector(".read-page").classList.add("under-hero"); }
    rows.replaceChildren(...got.filter((r) => r.items.length).map((r) => (r.id === "trending"
      ? row("top10", "Top 10 today", r.items.slice(0, 10).map((c, i) => topTen(c, i + 1)), "trending")
      : row(r.id, r.title, r.items.map((c) => card(c))))));
    // Picked from what you read, right under the Top 10, like Movies' "Because you watched".
    forYou().then((fy) => {
      if (!alive(my) || !fy.length) return;
      const at = rows.children[1] || null;
      for (const r of fy) rows.insertBefore(row(r.id, r.title, r.items.map((c) => card(c))), at);
    });
    invoke("read_book_rows").then((bk) => {
      if (!alive(my)) return;
      rows.append(...bk.filter((r) => r.items.length).slice(0, 2).map((r) => row(r.id, r.title, r.items.map((c) => card(c)))));
    }).catch(() => {});
  } catch (e) {
    if (alive(my)) rows.replaceChildren(note(String(e)));
  }
  return true;
}

/** Manga, Manhwa, Manhua, Light novels: a grid with filters, more as you scroll. */
/* Filters, like Movies': genres and tags in or out (click once for only these,
   again to leave them out, again to clear), status, rating, years, length and,
   for the PC owner or a Jellyfin admin, 18+. Kept per profile, per tab. */
const BLANK_FILTER = { genres: [], not_genres: [], tags: [], not_tags: [], status: null, sort: "popular", min_score: null, year_from: null, year_to: null, length: 0, adult: "hide", porn: "hide" };
const LENGTHS = [[null, null, "Any length"], [null, 50, "Up to 50"], [50, 150, "50 to 150"], [150, null, "150 or more"]];
const SCORES = [[null, "Any"], [60, "6+"], [70, "7+"], [80, "8+"], [90, "9+"]];
const STATUSES = [[null, "Any"], ["RELEASING", "Ongoing"], ["FINISHED", "Finished"], ["HIATUS", "On hiatus"], ["NOT_YET_RELEASED", "Coming soon"]];
const SORTS = [["popular", "Most popular"], ["trending", "Trending"], ["score", "Top rated"], ["new", "Newest"], ["title", "A to Z"]];
let filterInfo = null;

function filterCount(f) {
  return f.genres.length + f.not_genres.length + f.tags.length + f.not_tags.length + (f.status ? 1 : 0) + (f.min_score ? 1 : 0)
    + (f.year_from || f.year_to ? 1 : 0) + (f.length ? 1 : 0) + (f.adult !== adultDefault() ? 1 : 0) + (pornOn(f) ? 1 : 0);
}

async function explorePage(view, tab, my) {
  clearInterval(heroTimer);
  const kind = tab.slice(2);
  const saved = (() => { try { return JSON.parse(getPref(`read-filter-${kind}`) || "{}"); } catch (e) { return {}; } })();
  // Saved before 18+ started Included (no v): its "Not included" was only the old default.
  if (!saved.v) { delete saved.adult; delete saved.porn; }
  const f = { ...BLANK_FILTER, adult: adultDefault(), ...saved, v: 2 };
  const [genres, info] = await Promise.all([invoke("read_genres").catch(() => []), filterInfo || invoke("read_filter_info").catch(() => ({ tags: [], adult: false }))]);
  filterInfo = info;
  if (!alive(my)) return;
  // Only the PC owner or a Jellyfin admin gets 18+; the PC refuses it for anyone else too.
  if (!info.adult) { f.adult = "hide"; f.porn = "hide"; }
  const grid = el("div", { class: "grid read-grid" });
  const more = el("div", { class: "read-more" });
  const panel = el("div", { class: "fl-bar read-fl" });
  // Like Movies' tabs: rows (trending, popular, by genre...) until a filter or
  // another order is picked; then every match, more as you scroll.
  const rowsBox = el("div", { class: "read-kind-rows" }, ...skel());
  const filteredNote = el("div", { class: "fl-tab-note read-kind-note" });
  const filtering = () => filterCount(f) > 0 || f.sort !== "popular";
  let rowsAsked = false;
  function showMode() {
    const on = filtering();
    rowsBox.hidden = on;
    grid.hidden = !on;
    more.hidden = !on;
    filteredNote.hidden = !on;
    filteredNote.replaceChildren(...(on ? [el("span", { text: "Showing what matches your filters." }),
      el("button", { class: "linkish", onclick: () => change((x) => Object.assign(x, { ...BLANK_FILTER, adult: info.adult ? adultDefault() : "hide" })) }, "Back to all rows")] : []));
    if (!on && !rowsAsked) {
      rowsAsked = true;
      remembered(`kr:${kind}`, 30 * 60000, () => invoke("read_kind_rows", { kind })).then((rows) => {
        if (!alive(my)) return;
        rowsBox.replaceChildren(...rows.map((r) => (r.id === "trending" && styleNow() === "cinema"
          ? row(`k.${kind}.top10`, "Top 10 today", r.items.slice(0, 10).map((c, i) => topTen(c, i + 1)), `k.${kind}.trending`)
          : row(`k.${kind}.${r.id}`, r.title, r.items.map((c) => card(c))))));
      }).catch((e) => { rowsAsked = false; if (alive(my)) rowsBox.replaceChildren(note(String(e)), el("button", { class: "btn soft", style: "margin: 0 var(--gutter)", onclick: () => { rowsBox.replaceChildren(...skel()); showMode(); } }, "Try again")); });
    }
  }
  let moreOpen = false;
  const { button: toggle, count: showCount } = filtersButton(panel, "read-filters-open");
  const order = orderSelect(SORTS, f.sort, (v) => change((x) => (x.sort = v)));

  function change(mut) {
    mut(f);
    drawPanel();
    restart();
  }
  const kit = chipKit(f, change);
  const { triChip, oneOf, field, years } = kit;
  function drawPanel() {
    const n = filterCount(f);
    showCount(n);
    const gs = pornOn(f) && info.adult ? [...genres, "Hentai"] : genres;
    const parts = [
      field("Genres", el("div", { class: "chips" }, gs.map((g) => triChip(g, "genres", "not_genres")))),
      // AniList has 18 manga genres; its most used tags carry the rest.
      field("Themes", el("div", { class: "chips" }, info.tags.slice(0, 20).map((t) => triChip(t, "tags", "not_tags")))),
      el("div", { class: "fl-row" },
        field("Status", oneOf(STATUSES, (o) => f.status === o[0], (x, o) => (x.status = o[0]))),
        field("Rating", oneOf(SCORES, (o) => f.min_score === o[0], (x, o) => (x.min_score = o[0])))),
      moreButton(moreOpen, () => { moreOpen = !moreOpen; drawPanel(); }),
    ];
    if (moreOpen) {
      parts.push(
        field("More themes", el("div", { class: "chips" }, info.tags.slice(20).map((t) => triChip(t, "tags", "not_tags")))),
        el("div", { class: "fl-row" },
          field("Length", oneOf(LENGTHS.map((l, i) => [i, l[2]]), (o) => f.length === o[0], (x, o) => (x.length = o[0]))),
          field("Years", years())));
      if (info.adult) {
        parts.push(...adultFields(f, kit, true));
      }
    }
    if (n) parts.push(clearButton(() => change((x) => Object.assign(x, { ...BLANK_FILTER, adult: info.adult ? adultDefault() : "hide", sort: x.sort }))));
    panel.replaceChildren(...parts);
  }
  drawPanel();
  // The top of the tab takes on the style. Cinema: the big picture, this tab's
  // trending series. Midnight: a smaller one, and the ranking down the side.
  // Classic: a banner and a numbered top 5. Whatever the filters say.
  const style = styleNow();
  const top = el("div", { class: "read-hero-slot" });
  const filterHead = head(TAB_TITLE[tab], el("div", { class: "read-filters" }, toggle, order));
  const picker = inlinePicker();
  let body;
  if (style === "midnight") {
    const side = el("aside", { class: "mn-side" }, el("h2", { class: "mn-h", text: "Popular" }), tabbed("mn-rank", [
      ["Trending", () => exploreItems({ kind, sort: "trending" }).then((x) => x.slice(0, 10))],
      ["Top rated", () => exploreItems({ kind, sort: "score" }).then((x) => x.slice(0, 10))],
      ["Most read", () => exploreItems({ kind, sort: "popular" }).then((x) => x.slice(0, 10))],
    ], (items) => items.map(rankRow)));
    body = el("div", { class: "read-page mn-home mn-explore" }, picker, filterHead, el("div", { class: "mn-cols" }, el("div", { class: "mn-main" }, panel, filteredNote, rowsBox, grid, more), side));
  } else if (style === "classic") {
    const top5 = el("section", { class: "cl-block cl-top5" }, tabbed("cl-top", [
      ["Trending", () => exploreItems({ kind, sort: "trending" }).then((x) => x.slice(0, 5))],
      ["Top rated", () => exploreItems({ kind, sort: "score" }).then((x) => x.slice(0, 5))],
    ], (items) => [el("div", { class: "cl-ranks" }, items.map(classicRank))]));
    body = el("div", { class: "read-page cl-home cl-explore" }, picker,
      el("div", { class: "cl-banner" }, el("h1", { text: TAB_TITLE[tab] }), el("p", { text: `The ${TAB_TITLE[tab].toLowerCase()} people are reading now, and everything else to find.` })),
      top5, filterHead, panel, filteredNote, rowsBox, grid, more);
  } else {
    body = page(picker, filterHead, panel, filteredNote, rowsBox, grid, more);
  }
  view.replaceChildren(top, body);
  if (style !== "classic") {
    invoke("read_explore", { e: { kind, sort: "trending", page: 1 } }).then((g) => {
      if (!alive(my)) return;
      const h = readHero(heroPicks(g.items || []));
      if (h) { if (style === "midnight") h.classList.add("compact"); top.replaceWith(h); body.classList.add("after-hero"); }
    }).catch(() => {});
  }
  await know();
  let pageNo = 0, done = false, busy = false, run = 0;
  async function next() {
    if (busy || done || !alive(my) || !filtering()) return;
    busy = true;
    const mine = run;
    more.replaceChildren(el("div", { class: "skel skel-row" }));
    try {
      const [cmin, cmax] = LENGTHS[f.length] || LENGTHS[0];
      const got = await invoke("read_explore", { e: { kind, genres: f.genres, not_genres: f.not_genres, tags: f.tags, not_tags: f.not_tags, status: f.status, sort: f.sort,
        min_score: f.min_score, year_from: f.year_from, year_to: f.year_to, chapters_min: cmin, chapters_max: cmax, adult: f.adult, porn: f.porn, page: ++pageNo } });
      if (!alive(my) || mine !== run) {
        // Left while it loaded: that page is asked for again when the grid comes back.
        if (mine === run) pageNo--;
        return;
      }
      // Each new page's covers come in one after another.
      const start = grid.childElementCount;
      grid.append(...(got.items || []).map((c, k) => { const t = card(c); t.style.setProperty("--i", String(Math.min(k, 20))); t.classList.add("rise"); return t; }));
      if (!start) grid.scrollTop = 0;
      done = !got.more;
      more.replaceChildren(done ? note(grid.childElementCount ? "That's everything." : "Nothing matches these filters.") : "");
      // Still at the bottom (a tall screen, or the page came in while it was
      // already showing): the watcher won't fire again, so ask for the next now.
      if (!done) setTimeout(() => { if (alive(my) && mine === run && more.getBoundingClientRect().top < innerHeight + 900) next(); }, 0);
    } catch (e) {
      if (alive(my) && mine === run) { more.replaceChildren(note(String(e))); done = true; }
    } finally { if (mine === run) busy = false; }
  }
  function restart() {
    setPref(`read-filter-${kind}`, JSON.stringify(f));
    run++; pageNo = 0; done = false; busy = false;
    grid.replaceChildren();
    showMode();
    next();
  }
  const io = new IntersectionObserver((e) => { if (e[0].isIntersecting) next(); }, { rootMargin: "900px" });
  // Watches the bottom while this page is on screen; stops when you leave.
  const watch = () => {
    io.observe(more);
    window.addEventListener("hashchange", () => { if (!alive(my)) io.disconnect(); }, { once: true });
  };
  watch();
  // Back from being kept (renderRead): the watcher fires at once if the bottom is in view.
  body.resume = watch;
  showMode();
  await next();
  return true;
}

/** Every title in a row, more as you scroll: a reading row's "Explore all". */
async function allPage(view, my) {
  clearInterval(heroTimer);
  const [, rawId, rawTitle] = location.hash.slice(1).split("/");
  const id = decodeURIComponent(rawId || "");
  const title = decodeURIComponent(rawTitle || "") || "Explore";
  const books = id.startsWith("books-") || id.startsWith("comics-");
  const grid = el("div", { class: "grid read-grid" });
  const more = el("div", { class: "read-more" });
  const back = el("button", { class: "btn soft read-back", onclick: () => (history.length > 1 ? history.back() : (location.hash = "#r-home")) }, icon(ICON.left), "Back");
  view.replaceChildren(page(inlinePicker(), head(title, back), grid, more));
  if (!hasAll(id)) { more.replaceChildren(note("This row can't be opened.")); return true; }
  await know();
  if (!alive(my)) return;
  let pageNo = 0, done = false, busy = false;
  async function next() {
    if (busy || done || !alive(my)) return;
    busy = true;
    more.replaceChildren(el("div", { class: "skel skel-row" }));
    try {
      ++pageNo;
      // "Because you read X": AniList's whole list of picks, one page. "More Action for you": that genre, most read first.
      const because = /^because-(\d+)$/.exec(id);
      const genre = /^foryou-(.+)$/.exec(id);
      const got = books ? await invoke("read_book_more", { id, page: pageNo })
        : because ? { items: pageNo > 1 ? [] : await invoke("read_similar", { anilist: Number(because[1]) }), more: false }
          : await invoke("read_explore", { e: { ...(genre ? { kind: "all", genres: [genre[1]], sort: "popular" } : kindRowQuery(id) || ROW_EXPLORE[id]), page: pageNo } });
      if (!alive(my)) return;
      grid.append(...(got.items || []).map((c, k) => { const t = card(c); t.style.setProperty("--i", String(Math.min(k, 20))); t.classList.add("rise"); return t; }));
      done = !got.more;
      more.replaceChildren(done ? note(grid.childElementCount ? "That's everything." : "Nothing here right now.") : "");
      if (!done) setTimeout(() => { if (alive(my) && more.getBoundingClientRect().top < innerHeight + 900) next(); }, 0);
    } catch (e) {
      if (alive(my)) { more.replaceChildren(note(String(e))); done = true; }
    } finally { busy = false; }
  }
  const io = new IntersectionObserver((e) => { if (e[0].isIntersecting) next(); }, { rootMargin: "900px" });
  io.observe(more);
  window.addEventListener("hashchange", () => io.disconnect(), { once: true });
  await next();
  return true;
}

/** Every book by one author, opened from the author's name on a book. Looks like the watching side's maker page. */
async function authorPage(view, my) {
  clearInterval(heroTimer);
  const name = decodeURIComponent(location.hash.slice(1).split("/")[1] || "");
  const grid = el("div", { class: "grid read-grid" });
  const more = el("div", { class: "read-more" }, el("div", { class: "skel skel-row" }));
  const back = el("button", { class: "btn soft read-back", onclick: () => (history.length > 1 ? history.back() : (location.hash = "#r-books")) }, icon(ICON.left), "Back");
  view.replaceChildren(page(inlinePicker(), el("header", { class: "read-head maker-head" }, back, el("div", {}, el("h1", { text: name }), el("p", { class: "maker-sub", text: "All their books" }))), grid, more));
  let books;
  try { books = await invoke("read_book_author", { name }); }
  catch (e) { if (alive(my)) more.replaceChildren(note(String(e))); return true; }
  if (!alive(my)) return;
  grid.append(...books.map((c, k) => { const t = card(c); t.style.setProperty("--i", String(Math.min(k, 20))); t.classList.add("rise"); return t; }));
  more.replaceChildren(note(books.length ? (books.length === 1 ? "That's the only book found." : `That's all ${books.length} books found.`) : "No books found for this author."));
  return true;
}

async function booksPage(view, my) {
  const mine = el("div");
  const rows = el("div", {}, ...skel());
  view.replaceChildren(page(head("Books"), mine, rows));
  myBooks = await invoke("read_books").catch(() => []);
  if (!alive(my)) return;
  if (myBooks.length) mine.replaceChildren(withSelect(row("books", "Your books", myBooks.map(bookCard), "mine"), "books"));
  const comics = el("div");
  // Comics and graphic novels come after the books, as soon as they're in.
  const cm = invoke("read_comic_rows").catch(() => []);
  const bk = await invoke("read_book_rows").catch(() => []);
  if (!alive(my)) return;
  rows.replaceChildren(...(bk.filter((r) => r.items.length).map((r) => row(r.id, r.title, r.items.map((c) => card(c))))), comics);
  // From your own books: more by the authors you read most lately, right under your books.
  const authors = [...new Set(myBooks.slice().sort((a, b) => (b.added || 0) - (a.added || 0)).map((b) => b.author).filter(Boolean))].slice(0, 2);
  Promise.all(authors.map((name) => remembered(`ba:${name}`, 30 * 60000, () => invoke("read_book_author", { name })).then((list) => ({ name, list })).catch(() => null))).then((got) => {
    if (!alive(my)) return;
    const have = new Set(myBooks.map((b) => (b.title || "").toLowerCase()));
    const made = got.filter(Boolean).map(({ name, list }) => ({ name, list: (list || []).filter((c) => !have.has((c.title || "").toLowerCase())) })).filter((x) => x.list.length >= 2)
      .map(({ name, list }) => {
        const r = row(`by-${name}`, `More by ${name}`, list.map((c) => card(c)));
        const head = r.querySelector(".row-head");
        head.querySelector(".count")?.replaceWith(el("a", { class: "explore", href: `#r-author/${encodeURIComponent(name)}` }, "See all", icon(ICON.right)));
        return r;
      });
    rows.prepend(...made);
  });
  const cr = await cm;
  if (!alive(my)) return;
  comics.replaceChildren(...(cr.filter((r) => r.items.length).map((r) => row(r.id, r.title, r.items.map((c) => card(c))))));
  if (!rows.querySelector(".track")) rows.replaceChildren(note("Open Library isn't answering. Try again in a minute."));
  return true;
}

async function listPage(view, my) {
  const body = el("div", {}, ...skel());
  view.replaceChildren(page(head("My List"), body));
  const [{ lib, list }, books] = await Promise.all([know(), invoke("read_books").catch(() => [])]);
  if (!alive(my)) return;
  const out = [];
  if (list.length) out.push(expandRow(row("saved", "Saved for later", list.map((c) => card(c))), "r-saved"));
  if (lib.length) out.push(withSelect(expandRow(row("following", "Following", lib.map((s) => card(asCard(s), s))), "r-following"), "series"));
  if (books.length) out.push(withSelect(expandRow(row("books", "Books", books.map(bookCard), "mine"), "r-books"), "books"));
  body.replaceChildren(...(out.length ? out : [note("Nothing here yet. Press Add to My List on anything you'd like to read later.")]));
  return true;
}

async function downloadsPage(view, my) {
  const body = el("div", {}, ...skel());
  view.replaceChildren(page(head("Downloads"), body));
  // Rows by key: a new look changes the bars and counts in place, and only a
  // series or book starting or finishing changes the list. Chapters move by
  // read-live (readlive.rs); books (torrents) are looked at every 4 s while
  // one downloads and the page is on screen.
  const rows = new Map();
  let timer = 0;
  let busy = false;
  const seriesRow = (s) => {
    const bar = el("span");
    const count = el("small");
    const n = el("div", { class: "read-dl" },
      el("img", { class: "read-dl-cover", src: s.cover || "", alt: "" }),
      el("div", { class: "read-dl-text" }, el("strong", { text: s.title }), count, el("div", { class: "read-q wide" }, bar)),
      el("button", { class: "btn soft small", onclick: async () => { try { toast(await invoke("read_stop_chapters", { key: s.key })); draw(); } catch (e) { toast(String(e), true); } } }, "Stop"));
    n.set = (x) => { count.textContent = `${x.chapters} chapter${x.chapters === 1 ? "" : "s"} to go`; bar.style.width = `${Math.max(3, Math.round(x.progress * 100))}%`; };
    return n;
  };
  const bookRow = (b) => {
    const bar = el("span");
    const what = el("small");
    const n = el("div", { class: "read-dl" },
      el("img", { class: "read-dl-cover", src: b.cover || "", alt: "" }),
      el("div", { class: "read-dl-text" }, el("strong", { text: b.title }), what, el("div", { class: "read-q wide" }, bar)));
    n.set = (x) => { what.textContent = x.problem || (x.progress != null ? `${Math.round(x.progress * 100)}%` : "Starting"); bar.style.width = `${Math.max(3, Math.round((x.progress || 0) * 100))}%`; };
    return n;
  };
  const draw = async () => {
    if (busy) return;
    busy = true;
    clearTimeout(timer);
    const d = await invoke("read_downloads").catch((e) => ({ error: String(e), series: [], books: [] }));
    busy = false;
    if (!alive(my)) return;
    const want = [...d.series.map((x) => ["s" + x.key, x, seriesRow]), ...d.books.map((x) => ["b" + x.key, x, bookRow])];
    const same = want.length === rows.size && want.every(([k]) => rows.has(k));
    if (!same) {
      rows.clear();
      for (const [k, x, make] of want) rows.set(k, make(x));
      body.replaceChildren(...(want.length ? rows.values() : [note(d.error || "Nothing is downloading.")]));
    }
    for (const [k, x] of want) rows.get(k).set(x);
    if (d.books.length) timer = setTimeout(function again() { if (!alive(my)) return; if (document.hidden) { timer = setTimeout(again, 4000); return; } draw(); }, 4000);
  };
  // Chapters moved: looked at again, a little after the last change.
  let soon = 0;
  const onLive = () => { if (!alive(my)) { removeEventListener("chillarr:read-live", onLive); return; } clearTimeout(soon); soon = setTimeout(draw, 400); };
  addEventListener("chillarr:read-live", onLive);
  await draw();
  return true;
}

/** The top bar's search, in reading mode. */
/** This profile turned 18+ on in any reading tab's filters (Included or Only),
    and, with `porn`, Porn too in that same tab. */
function adultOn(porn) {
  return ["manga", "manhwa", "manhua", "novels"].some((k) => {
    try { const f = JSON.parse(getPref(`read-filter-${k}`) || "{}"); if (!f.v) { delete f.adult; delete f.porn; } f.adult = adultOf(f); return porn ? adultShows(f.adult) && pornOn(f) : adultShows(f.adult); } catch (e) { return false; }
  });
}

export async function renderReadSearch(view, term) {
  // The tab under the search is kept now, while it's still on screen. Kept
  // later (when the box is cleared), the search results were kept in its
  // place and came straight back instead of the tab.
  stash();
  alias = -1;
  const my = ++seq;
  const body = el("div", {}, ...skel());
  view.replaceChildren(page(head(`"${term}"`), body));
  const [found, books, comics] = (await Promise.all([
    invoke("read_search", { term, adult: adultOn(false), porn: adultOn(true) }).catch(() => []),
    invoke("read_book_search", { term }).catch(() => []),
    invoke("read_comic_search", { term }).catch(() => []),
  ])).map((x) => x || []);
  if (!alive(my)) return;
  await know();
  const out = [];
  if (found.length) out.push(el("h2", { class: "read-grid-title", text: "Manga, manhwa, manhua and novels" }), el("div", { class: "grid read-grid" }, found.map((c) => card(c))));
  if (books.length) out.push(el("h2", { class: "read-grid-title", text: "Books" }), el("div", { class: "grid read-grid" }, books.map((c) => card(c))));
  if (comics.length) out.push(el("h2", { class: "read-grid-title", text: "Comics and graphic novels" }), el("div", { class: "grid read-grid" }, comics.map((c) => card(c))));
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
  dlg.addEventListener("close", () => {
    clearTimeout(poll);
    panelSeq++;
    if (staleBehind) { staleBehind = false; window.dispatchEvent(new Event("chillarr:refresh")); }
  });
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
  // Hide is on every series and book: it goes from every page of this profile.
  const hideOn = () => actions.append(hideButton(c.title, () => { if (dlg) dlg.close(); }, c));
  const poster = c.cover ? el("img", { class: "read-poster", src: c.cover, alt: "" }) : null;
  hero.append(el("button", { class: "close", "aria-label": "Close", onclick: () => d.close() }, icon("M6 6l12 12M18 6L6 18")),
    el("div", { class: "read-head-row" }, poster, el("div", { class: "detail-head" }, el("h2", { text: c.title }), meta, options, actions)));
  const about = aboutBox(c);
  if (c.anilist && !isBook(c)) meta.parentNode.insertBefore(makersLine(c), meta.nextSibling);
  const body = el("div", { class: "detail-body" }, c.description ? el("p", { class: "overview read-overview", text: c.description }) : null, about);
  d.replaceChildren(hero, body);
  if (!d.open) {
    // The entrance plays when it opens, not on every refresh while chapters come in.
    d.classList.add("fresh");
    clearTimeout(d.freshT);
    d.freshT = setTimeout(() => d.classList.remove("fresh"), 1200);
    d.showModal();
  }
  return { actions, body, options, hideOn, about };
}

/* ---------- who made it (issue: "Same author") ----------
   Like a film's director: the writer and the artist of a series are links,
   each opening everything else they made (r-maker), and the panel shows a
   row of their other series. AniList's staff, asked once per series. */
const makersSeen = new Map();
function makersOf(anilist) {
  if (!makersSeen.has(anilist)) makersSeen.set(anilist, invoke("read_makers", { anilist }).catch(() => { makersSeen.delete(anilist); return []; }));
  return makersSeen.get(anilist);
}
const makerHref = (m) => `#r-maker/${m.id}/${encodeURIComponent(m.name)}`;

/** "Story and Art: Chugong · Art: DUBU", each name a link. Fills in when AniList answers. */
function makersLine(c) {
  const line = el("p", { class: "read-by", hidden: true });
  makersOf(c.anilist).then((list) => {
    if (!list.length) return;
    const parts = [];
    list.slice(0, 3).forEach((m, i) => {
      if (i) parts.push(el("span", { class: "sep" }));
      parts.push(el("span", { class: "read-by-role", text: `${m.role} ` }),
        el("button", { class: "maker-link", title: `Everything by ${m.name}`, onclick: () => { if (dlg) dlg.close(); location.hash = makerHref(m); } }, m.name));
    });
    line.replaceChildren(...parts);
    line.hidden = false;
  });
  return line;
}

/** "More by <writer>" under a series: their other series, this one left out. */
function moreBySection(c, mine) {
  if (!c || !c.anilist || isBook(c)) return null;
  const box = el("section", { class: "read-similar read-moreby", hidden: true });
  makersOf(c.anilist).then(async (list) => {
    const m = list[0];
    if (!m) return;
    const works = (await remembered(`mw:${personKey()}:${m.id}:${adultOn(false)}`, 30 * 60000, () => invoke("read_maker_works", { staff: m.id, adult: adultOn(false) })).catch(() => []))
      .filter((x) => x.anilist !== c.anilist);
    if (mine !== panelSeq || works.length < 1) return;
    box.replaceChildren(el("div", { class: "read-moreby-head" }, el("h3", { class: "read-similar-h", text: `More by ${m.name}` }),
      el("a", { class: "explore", href: makerHref(m), onclick: () => { if (dlg) dlg.close(); } }, "See all", icon(ICON.right))),
    el("div", { class: "read-similar-grid" }, works.slice(0, 12).map((x, i) => { const t = card(x); t.classList.add("rise"); t.style.setProperty("--i", String(i)); return t; })));
    box.hidden = false;
  });
  return box;
}

/** Everything one writer or artist made, from their name on a series. */
async function makerPage(view, my) {
  clearInterval(heroTimer);
  const [, rawId, rawName] = location.hash.slice(1).split("/");
  const id = Number(rawId) || 0;
  const name = decodeURIComponent(rawName || "");
  const grid = el("div", { class: "grid read-grid" });
  const more = el("div", { class: "read-more" }, el("div", { class: "skel skel-row" }));
  const back = el("button", { class: "btn soft read-back", onclick: () => (history.length > 1 ? history.back() : (location.hash = "#r-home")) }, icon(ICON.left), "Back");
  view.replaceChildren(page(el("header", { class: "read-head maker-head" }, back, el("div", {}, el("h1", { text: name }), el("p", { class: "maker-sub", text: "Everything they wrote or drew" }))), grid, more));
  let works;
  try { [works] = await Promise.all([invoke("read_maker_works", { staff: id, adult: adultOn(false) }), know()]); }
  catch (e) { if (alive(my)) more.replaceChildren(note(String(e))); return true; }
  if (!alive(my)) return;
  grid.append(...works.map((c, k) => { const t = card(c); t.style.setProperty("--i", String(Math.min(k, 20))); t.classList.add("rise"); return t; }));
  more.replaceChildren(note(works.length ? (works.length === 1 ? "That's the only series found." : `That's all ${works.length} found.`) : "Nothing found for them yet."));
  return true;
}

/** Details asked for lately, so a panel redrawing itself doesn't ask again. */
const aboutSeen = new Map();

/**
 * The details under the description, like a film's: genres, who made it, how
 * many chapters and volumes, the latest chapter and when it came out, the
 * status and the years. AniList for series, Open Library for books. The latest
 * chapter comes from the chapter list, once there is one (`about.latest(chs)`).
 */
function aboutBox(c) {
  const box = el("dl", { class: "facts read-facts" });
  box.hidden = true;
  let facts = [];
  let latest = null;
  const draw = () => {
    const rows = facts.slice();
    if (latest) {
      const at = rows.findIndex(([k]) => k === "Status");
      const extra = [["Latest chapter", latest.text]];
      if (latest.count && !rows.some(([k]) => k === "Chapters")) extra.unshift(["Chapters", `${latest.count} so far`]);
      rows.splice(at < 0 ? Math.min(1, rows.length) : at + 1, 0, ...extra);
    }
    box.replaceChildren(...rows.map(([k, v]) => el("div", {}, el("dt", { text: k }), el("dd", { text: v }))));
    box.hidden = !rows.length;
  };
  box.latest = (chs) => {
    const nums = (chs || []).filter((r) => r.number != null);
    if (!nums.length) return;
    const top = nums.reduce((a, b) => (b.number > a.number ? b : a));
    const when = ago(top.uploaded);
    latest = { text: when ? `${chLabel(top)}, ${when.charAt(0).toLowerCase()}${when.slice(1)}` : chLabel(top), count: chs.length };
    draw();
  };
  const key = c.anilist ? `a${c.anilist}` : c.olid ? `o${c.olid}` : null;
  if (!key) return box;
  const seen = aboutSeen.get(key);
  if (seen) { facts = seen; draw(); return box; }
  invoke("read_about", { anilist: c.anilist || null, olid: c.anilist ? null : c.olid || null }).then((got) => {
    aboutSeen.set(key, got || []);
    facts = got || [];
    if (box.isConnected) draw();
  }).catch(() => {});
  return box;
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
  if (!key && c && followedOf(c)) key = followedOf(c);
  if (key) return showFollowed(key, mine, c);
  const { actions, body, options, hideOn, about } = shell(c);
  // The chapters show by themselves: the copies are looked up as the panel opens,
  // and again when the translation or copy choice changes.
  const state = { lang: getPref("read-lang") || "en", copy: getPref("read-copy") || "best", onChange: () => getCopies(c, state, list, mine) };
  pickers(options, state);
  state.about = about;
  // Follow: new chapters come in from now on, nothing older is downloaded unless picked below.
  const followBtn = el("button", { class: "btn primary", disabled: true, title: "Finding the chapters first" }, icon(ICON.plus), "Follow");
  followBtn.addEventListener("click", () => state.follow && state.follow([], followBtn));
  state.followBtn = followBtn;
  actions.append(followBtn, listButton(c));
  hideOn();
  const list = el("section", { class: "read-sources" });
  body.append(...[list, moreBySection(c, mine), similarSection(c, mine)].filter(Boolean));
  getCopies(c, state, list, mine);
}

/** Copies found lately, so opening a series again doesn't ask every site again. */
const copiesSeen = new Map();
const COPIES_LIFE = 20 * 60 * 1000;

/** "Similar to this" under a series: what AniList's readers recommend. Kept per
 *  series, since the panel redraws itself while chapters come in. */
const similarSeen = new Map();
function similarSection(c, mine) {
  if (!c || !c.anilist) return null;
  const head = () => el("h3", { class: "read-similar-h", text: "Similar to this" });
  const box = el("section", { class: "read-similar" }, head(), el("div", { class: "skel skel-row" }));
  const draw = (list) => {
    if (!list.length) { box.remove(); return; }
    box.replaceChildren(head(), el("div", { class: "read-similar-grid" }, list.slice(0, 12).map((x, i) => {
      const t = card(x);
      t.classList.add("rise");
      t.style.setProperty("--i", String(i));
      return t;
    })));
  };
  const seen = similarSeen.get(c.anilist);
  if (seen) { draw(seen); return box; }
  invoke("read_similar", { anilist: c.anilist }).then((list) => {
    similarSeen.set(c.anilist, list);
    if (mine === panelSeq) draw(list);
  }).catch(() => box.remove());
  return box;
}

/** Finds the copies in the chosen translation; Best goes straight to its chapters. */
async function getCopies(c, state, list, mine) {
  // A newer ask (the translation changed) wins over one still out.
  state.asks = (state.asks || 0) + 1;
  const ask = state.asks;
  // Follow waits for this answer: until then it would follow the copy found
  // for the translation picked before.
  state.follow = null;
  if (state.followBtn) { state.followBtn.disabled = true; state.followBtn.title = "Finding the chapters first"; }
  list.replaceChildren(el("p", { class: "note read-wait", text: "Finding the chapters on every site, sharpest first. This can take up to a minute." }));
  const seenKey = `${c.anilist || c.title}|${state.lang}`;
  let seen = copiesSeen.get(seenKey);
  // The search itself is kept, not just its answer: opening the series again
  // (or flipping Best and Let me choose) while it runs joins it instead of
  // asking every site again.
  if (!seen || Date.now() - seen.at >= COPIES_LIFE) {
    seen = { at: Date.now(), p: invoke("read_sources", { meta: meta(c), lang: state.lang }) };
    copiesSeen.set(seenKey, seen);
    if (copiesSeen.size > 40) copiesSeen.delete(copiesSeen.keys().next().value);
  }
  let got;
  try {
    got = await seen.p;
  } catch (e) {
    if (copiesSeen.get(seenKey) === seen) copiesSeen.delete(seenKey);
    if (mine !== panelSeq || ask !== state.asks) return;
    list.replaceChildren(note(String(e)), el("button", { class: "btn soft", onclick: () => getCopies(c, state, list, mine) }, "Try again"));
    return;
  }
  if (mine !== panelSeq || ask !== state.asks) return;
  const found = got.list;
  if (!found.length) {
    copiesSeen.delete(seenKey);
    list.replaceChildren(note(state.lang === "en" ? "No source has this one yet." : "No copy in that language yet. Try English."));
    return;
  }
  // Best: the sharpest copy, and the one furthest ahead filling in what it lacks.
  const best = found[got.best ?? 0];
  const fill = got.fill != null ? found[got.fill] : null;
  if (got.kept != null) state.kept = found[got.kept];
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
  // A newer ask (another translation picked meanwhile) wins over this one.
  const asked = state.asks;
  const stale = () => mine !== panelSeq || asked !== state.asks;
  let chs;
  try { chs = await invoke("read_copy_chapters", { mangaId: x.manga_id, fillId: fill ? fill.manga_id : null }); } catch (e) { if (!stale()) list.replaceChildren(note(String(e))); return; }
  if (stale()) return;
  if (state.about) state.about.latest(chs);
  // Nothing ticked to start with: tick what you want, or get them all.
  const picked = new Set();
  let last = null;
  const count = el("strong");
  const goBtn = el("button", { class: "btn primary" }, icon(ICON.download), "");
  const newToo = el("input", { type: "checkbox", class: "switch", "aria-label": "Also get new chapters" });
  const from = el("input", { class: "read-from", type: "number", min: "0", step: "1", placeholder: "1", "aria-label": "From chapter" });
  const rows = chs.map((ch, i) => {
    const box = el("input", { type: "checkbox", "aria-label": chLabel(ch) });
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
      releasedEl(ch.uploaded),
      ch.downloaded ? el("small", { text: "Here" }) : ch.fill && fill ? el("small", { text: srcName(fill.source) }) : null);
    r.box = box;
    return r;
  });
  function set(i, on) { rows[i].box.checked = on; if (on) picked.add(chs[i].id); else picked.delete(chs[i].id); }
  function paint() {
    const n = picked.size;
    count.textContent = n ? `${n} of ${chs.length} picked` : `${chs.length} chapters`;
    goBtn.lastChild.textContent = n === 0 || n === chs.length ? "Download all" : `Download ${n}`;
  }
  from.addEventListener("input", () => {
    const f = from.value === "" ? null : Number(from.value);
    chs.forEach((ch, i) => set(i, f == null || (ch.number ?? Infinity) >= f));
    paint();
  });
  const follow = async (ids) => {
    goBtn.disabled = true;
    justFollow.disabled = true;
    if (state.followBtn) state.followBtn.disabled = true;
    const sel = { ids, from: null, new_too: newToo.checked };
    try {
      toast(await invoke("read_follow", { meta: meta(c), pick: x, fill, sel, lang: state.lang }));
      await know();
      if (mine === panelSeq) openSeries({ key: `m${x.manga_id}`, card: c });
      window.dispatchEvent(new Event("chillarr:refresh"));
    } catch (e) { toast(String(e), true); goBtn.disabled = false; justFollow.disabled = false; if (state.followBtn) state.followBtn.disabled = false; }
  };
  state.follow = follow;
  if (state.followBtn) { state.followBtn.disabled = false; state.followBtn.title = ""; }
  // None ticked or all ticked: everything.
  goBtn.addEventListener("click", () => follow(picked.size === 0 || picked.size === chs.length ? null : [...picked]));
  const justFollow = el("button", { class: "btn soft", title: "Follow it without downloading anything yet", onclick: () => follow([]) }, "Just follow");
  const others = found.length > 1 ? el("button", { class: "linkish", onclick: () => chooseCopy(c, state, list, mine, found) }, "Other copies") : null;
  const filled = fill ? chs.filter((ch) => ch.fill).length : 0;
  // The notes that don't apply are null: left out, not drawn as "null".
  list.replaceChildren(...[
    el("div", { class: "read-sources-head" },
      el("h3", {}, `From ${srcName(x.source)}`, x.width ? el("small", { class: "read-px", text: ` ${x.width} px` }) : null), others),
    filled ? note(`${srcName(x.source)} is sharpest but stops early, so ${filled} chapter${filled === 1 ? " comes" : "s come"} from ${srcName(fill.source)}, which is further ahead.`) : null,
    state.kept === x && chs.some((ch) => ch.downloaded) ? note(`The ${chs.filter((ch) => ch.downloaded).length} chapters you kept are still here, so this copy is picked. Follow to read them again.`) : null,
    el("div", { class: "read-pick-bar" },
      el("label", { class: "read-from-wrap" }, "From chapter ", from),
      el("button", { class: "linkish", onclick: () => { chs.forEach((_, i) => set(i, true)); paint(); } }, "All"),
      el("button", { class: "linkish", onclick: () => { chs.forEach((_, i) => set(i, false)); paint(); } }, "None"),
      count),
    el("div", { class: "read-pick-list" }, rows.slice().reverse()),
    el("div", { class: "read-pick-go" }, el("label", { class: "read-newtoo" }, newToo, "Also get new chapters"), justFollow, goBtn)].filter(Boolean));
  paint();
}

const meta = (c) => ({ anilist: c.anilist ?? null, title: c.title, names: c.names || [], cover: c.cover ?? null, banner: c.banner ?? null, country: c.country ?? null, format: c.format ?? null, status: c.status ?? null, description: c.description ?? null, genres: c.genres || [], adult: !!c.adult });
const chLabel = (r) => (r.number != null ? `Chapter ${r.number}` : r.name || "Chapter");
/** When a chapter came out, as a short date ("11/09/26"); Suwayomi gives milliseconds, as text. */
function released(ms) {
  const t = Number(ms);
  if (!t) return null;
  const d = new Date(t);
  return { short: d.toLocaleDateString(undefined, { year: "2-digit", month: "2-digit", day: "2-digit" }), long: d.toLocaleDateString(undefined, { weekday: "long", year: "numeric", month: "long", day: "numeric" }) };
}
const releasedEl = (ms) => { const r = released(ms); return r ? el("small", { class: "read-ch-date", title: `Released ${r.long}`, text: `Released ${r.short}` }) : null; };

async function showFollowed(key, mine, c0) {
  let d;
  delOk.delete(key);
  try { [d] = await Promise.all([invoke("read_detail", { key }), mayDelete(key)]); } catch (e) { toast(String(e), true); return; }
  if (mine !== panelSeq) return;
  const f = d.followed;
  const c = { ...(c0 || {}), ...f, chapters: d.chapters.length };
  const scroll = dlg && dlg.open ? dlg.scrollTop : 0;
  const { actions, body, options, hideOn, about } = shell(c);
  about.latest(d.chapters);
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
  const isOn = f.new_too !== false;
  const live = el("button", { class: "btn soft read-live" + (isOn ? " on" : ""), "aria-pressed": isOn ? "true" : "false",
    title: "Checks the site every few minutes and downloads new chapters" }, icon(isOn ? ICON.check : "M12 7v5l3 2M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z"), isOn ? "Getting new chapters" : "Get new chapters");
  live.addEventListener("click", async () => {
    live.disabled = true;
    try {
      const on = await invoke("read_keep_up", { key, on: !isOn });
      toast(on ? "New chapters download by themselves. You get a notification when one is ready." : "New chapters are off for this series.");
      showFollowed(key, ++panelSeq, c0);
    } catch (e) { toast(String(e), true); live.disabled = false; }
  });
  actions.append(live, el("button", { class: "btn soft", onclick: (e) => unfollow(f, e.currentTarget) }, "Stop following"));
  hideOn();
  const hidden = !!(d.spot && d.spot.hide_week);
  actions.append(el("button", { class: "btn soft read-hideweek" + (hidden ? " on" : ""), "aria-pressed": String(hidden),
    title: "Keep it out of Your week and Latest updates on this profile",
    onclick: async () => { if (await setHideWeek(key, f.title, !hidden)) showFollowed(key, ++panelSeq, c0); } }, hidden ? "Show in Your week" : "Hide from Your week"));
  const here = d.chapters.filter((r) => r.downloaded);
  if (here.length && delOk.get(key)) actions.append(el("button", { class: "btn soft", onclick: () => deletePicker(key, f, here, body, c0) }, icon(TRASH), "Delete downloads"));
  const where = el("p", { class: "note read-from-source" },
    `From ${srcName(f.source)}`, f.width ? ` · ${f.width} px wide` : "",
    f.fill && f.fill.length ? ` · the rest from ${f.fill.map((x) => srcName(x.source)).join(", ")}` : "",
    d.downloading ? ` · getting ${d.downloading} chapter${d.downloading === 1 ? "" : "s"}` : "",
    f.new_too === false ? " · new chapters off" : "",
    " · ", el("button", { class: "linkish", onclick: () => changeCopy(c, f, body) }, "Change copy"));
  const list = el("ol", { class: "read-chapters" }, ...d.chapters.slice().reverse().map((r) => chapterRow(key, r, c0)));
  body.append(...[where, markBar(key, d.chapters, c0), list, moreBySection(c, mine), similarSection(c, mine)].filter(Boolean));
  if (scroll) dlg.scrollTop = scroll;
}

/** Marks a stretch of chapters read (read somewhere else, or skipped), or the whole series. */
function markBar(key, chs, c0) {
  if (!chs.length) return null;
  const nums = chs.map((r) => r.number).filter((n) => n != null);
  const lo = nums.length ? Math.min(...nums) : null;
  const hi = nums.length ? Math.max(...nums) : null;
  const box = (label, ph) => el("input", { class: "read-from", type: "number", step: "any", placeholder: ph != null ? String(ph) : "", "aria-label": label });
  const from = box("From chapter", lo);
  const to = box("To chapter", hi);
  const all = chs.every((r) => r.read);
  const send = async (ids, read, btn) => {
    if (!ids.length) { toast("No chapters in that stretch."); return; }
    btn.disabled = true;
    try {
      await invoke("read_mark", { key, chapters: ids, read });
      toast(read ? `Marked ${ids.length} chapter${ids.length === 1 ? "" : "s"} read.` : "Marked unread.");
      refreshIfOpen(key, c0);
    } catch (e) { toast(String(e), true); btn.disabled = false; }
  };
  // The chapters between the two boxes (either way round; empty means from the start or to the end).
  const pick = () => {
    const a = from.value === "" ? -Infinity : Number(from.value);
    const b = to.value === "" ? Infinity : Number(to.value);
    const [x, y] = a <= b ? [a, b] : [b, a];
    return chs.filter((r) => r.number != null && r.number >= x && r.number <= y);
  };
  const go = el("button", { class: "btn soft", onclick: (e) => send(pick().filter((r) => !r.read).map((r) => r.id), true, e.currentTarget) }, "Mark read");
  const get = el("button", { class: "btn soft", onclick: async (e) => {
    const ids = pick().filter((r) => !r.downloaded && r.queued == null).map((r) => r.id);
    if (!ids.length) { toast("Those are all downloaded already."); return; }
    const btn = e.currentTarget;
    btn.disabled = true;
    try { toast(await invoke("read_get_chapters", { key, ids })); refreshIfOpen(key, c0); } catch (err) { toast(String(err), true); btn.disabled = false; }
  } }, icon(ICON.download), "Download");
  const del = delOk.get(key) ? el("button", { class: "btn soft", onclick: async () => {
    const ids = pick().filter((r) => r.downloaded).map((r) => r.id);
    if (!ids.length) { toast("None of those are downloaded."); return; }
    if (await deleteChapters(key, ids)) refreshIfOpen(key, c0);
  } }, icon(TRASH), "Delete") : null;
  const whole = el("button", { class: "btn soft", onclick: (e) => send(all ? chs.map((r) => r.id) : chs.filter((r) => !r.read).map((r) => r.id), !all, e.currentTarget) },
    all ? "Mark all unread" : "Mark all read");
  return el("div", { class: "read-markbar" },
    el("span", { text: "Chapters" }), from, el("span", { text: "to" }), to, go, get, del, el("span", { class: "read-markbar-gap" }), whole);
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
  const li = el("li", { class: "read-ch" + (r.read ? " is-read" : "") + (r.downloaded ? "" : " not-here"), dataset: { ch: String(r.id) } },
    el("button", { class: "read-ch-open", disabled: !r.downloaded, onclick: () => read(key, r.id, c0) },
      el("strong", { text: chLabel(r) }), name ? el("span", { class: "read-ch-name", text: name }) : null,
      r.scanlator && !/^unknown$/i.test(r.scanlator) ? el("small", { text: r.scanlator }) : null,
      releasedEl(r.uploaded)),
    r.fresh ? el("b", { class: "read-new-pill", text: "NEW" }) : null,
    el("span", { class: "read-ch-state", text: state }), get || mark,
    r.downloaded && delOk.get(key) ? el("button", { class: "read-mark read-del", title: "Delete this chapter's download", "aria-label": `Delete ${chLabel(r)}`,
      onclick: async (e) => { e.stopPropagation(); if (await deleteChapters(key, [r.id], chLabel(r))) refreshIfOpen(key, c0); } }, icon(TRASH)) : null);
  // What a live update (read-live) needs to draw this row again on its own.
  li._row = { key, r, c0 };
  return li;
}

/* Chapter downloads, live (readlive.rs): the one row changes; the panel isn't
   drawn again (it used to be, every 5 seconds, which reloaded its pictures,
   moved the scroll and dropped what was being typed). */
let landedSoon = 0;
listen("read-live", (e) => {
  const { items, error } = e.payload || {};
  if (error) { if (dlg && dlg.open) toast(error, true); return; }
  for (const it of items || []) {
    const node = dlg && dlg.open ? dlg.querySelector(`.read-ch[data-ch="${it.ch}"]`) : null;
    if (!node || !node._row || node._row.key !== it.key) continue;
    const { key, r, c0 } = node._row;
    r.queued = it.done ? null : it.p;
    if (it.done) r.downloaded = true;
    node.replaceWith(chapterRow(key, r, c0));
  }
  window.dispatchEvent(new CustomEvent("chillarr:read-live", { detail: items || [] }));
  // A chapter landed: the reading tab under it shows it (what you follow is
  // asked again, softly), once things go quiet, and not under an open panel.
  if ((items || []).some((it) => it.done)) {
    clearTimeout(landedSoon);
    landedSoon = setTimeout(function again() {
      if (document.body.dataset.mode !== "read" || document.hidden) return;
      if (dlg && dlg.open) { landedSoon = setTimeout(again, 3000); return; }
      window.dispatchEvent(new Event("chillarr:refresh"));
    }, 3000);
  }
});

/* ---------- deleting downloads (issue #39) ---------- */
// The series stays followed: deleted chapters can be downloaded again, and
// new-chapter fetching leaves them alone.
const TRASH = "M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3";
export const bytesText = (b) => (b >= 1073741824 ? `${(b / 1073741824).toFixed(1)} GB` : b >= 1048576 ? `${Math.round(b / 1048576)} MB` : `${Math.max(1, Math.round((b || 0) / 1024))} KB`);

/** Asks first, with the size it frees and who else reads it; true once deleted. */
export async function deleteChapters(key, ids, what = "") {
  let plan;
  try { plan = await invoke("read_delete_plan", { key, ids }); } catch (e) { toast(String(e), true); return false; }
  if (!plan.chapters) { toast("Nothing of that is downloaded."); return false; }
  const n = plan.chapters;
  const head = n === 1 && what ? `Delete ${what} (${bytesText(plan.bytes)})?` : `Delete ${n} chapter${n === 1 ? "" : "s"} (${bytesText(plan.bytes)})?`;
  const others = plan.others || [];
  const body = others.length
    ? `${others.join(", ")} still read${others.length === 1 ? "s" : ""} this. The files go for them too. You can download them again any time.`
    : "You keep following it. You can download them again any time.";
  if (!(await ask(head, body, [], "Delete", "Cancel"))) return false;
  try { toast(await invoke("read_delete_chapters", { key, ids, othersOk: others.length > 0 })); return true; }
  catch (e) { toast(String(e), true); return false; }
}

/** Every downloaded chapter, ticked or not, with From/To and All/None. */
function deletePicker(key, f, here, body, c0) {
  const old = body.querySelector(".read-delpick");
  if (old) { old.remove(); return; }
  const picked = new Set(here.map((r) => r.id));
  const count = el("strong");
  const go = el("button", { class: "btn primary" }, icon(TRASH), "");
  let last = -1;
  const rows = here.map((r, i) => {
    const box = el("input", { type: "checkbox", checked: true, "aria-label": chLabel(r) });
    // Shift and a click: the whole stretch since the last one clicked.
    box.addEventListener("click", (e) => {
      const span = e.shiftKey && last >= 0 ? [Math.min(last, i), Math.max(last, i)] : [i, i];
      for (let j = span[0]; j <= span[1]; j++) { rows[j].box.checked = box.checked; if (box.checked) picked.add(here[j].id); else picked.delete(here[j].id); }
      last = i;
      paint();
    });
    const row = el("label", { class: "read-pick here" }, box, el("span", { class: "read-pick-n", text: chLabel(r) }), r.read ? el("small", { text: "Read" }) : null);
    row.box = box;
    return row;
  });
  const setAll = (on, test = () => true) => {
    here.forEach((r, i) => { const v = on && test(r); rows[i].box.checked = v; if (v) picked.add(r.id); else picked.delete(r.id); });
    paint();
  };
  const num = (label) => el("input", { class: "read-from", type: "number", step: "any", "aria-label": label });
  const from = num("From chapter");
  const to = num("To chapter");
  const range = () => {
    const a = from.value === "" ? -Infinity : Number(from.value);
    const b = to.value === "" ? Infinity : Number(to.value);
    const [x, y] = a <= b ? [a, b] : [b, a];
    setAll(true, (r) => (r.number == null ? x === -Infinity && y === Infinity : r.number >= x && r.number <= y));
  };
  from.addEventListener("input", range);
  to.addEventListener("input", range);
  function paint() {
    count.textContent = `${picked.size} of ${here.length} picked`;
    go.lastChild.textContent = picked.size === here.length ? "Delete all" : `Delete ${picked.size}`;
    go.disabled = !picked.size;
  }
  go.addEventListener("click", async () => {
    go.disabled = true;
    const ids = picked.size === here.length ? null : [...picked];
    if (await deleteChapters(key, ids)) refreshIfOpen(key, c0); else paint();
  });
  const box = el("section", { class: "read-sources read-delpick" },
    el("div", { class: "read-sources-head" }, el("h3", { text: `Delete downloads of ${f.title}` })),
    note("You keep following it. Deleted chapters can be downloaded again any time."),
    el("div", { class: "read-pick-bar" },
      el("label", { class: "read-from-wrap" }, "Chapters ", from), el("label", { class: "read-from-wrap" }, " to ", to),
      el("button", { class: "linkish", onclick: () => setAll(true) }, "All"),
      el("button", { class: "linkish", onclick: () => setAll(false) }, "None"),
      el("button", { class: "linkish", onclick: () => setAll(true, (r) => r.read) }, "Read ones"),
      count),
    el("div", { class: "read-pick-list" }, rows.slice().reverse()),
    el("div", { class: "read-pick-go" }, el("button", { class: "btn soft", onclick: () => box.remove() }, "Cancel"), go));
  body.prepend(box);
  paint();
  box.scrollIntoView({ behavior: "smooth", block: "start" });
}

/** A book's file goes; the book stays in the list as not downloaded. */
export async function deleteBookFile(x, others = []) {
  const body = others.length
    ? `${others.join(", ")} still read${others.length === 1 ? "s" : ""} this. The file goes for them too. The book stays in your list and you can download it again.`
    : "The book stays in your list as not downloaded. You can download it again any time.";
  if (!(await ask(`Delete the file of ${x.title}${x.bytes ? ` (${bytesText(x.bytes)})` : ""}?`, body, [], "Delete file", "Cancel"))) return false;
  try { toast(await invoke("read_book_delete_file", { key: x.key, othersOk: others.length > 0 })); return true; }
  catch (e) {
    // Asked from the book's own panel, which doesn't know who else has it.
    if (!others.length && /still reads/.test(String(e))
      && await ask("Someone else still reads this book", "The file goes for them too. Delete it anyway?", [], "Delete file", "Cancel")) {
      try { toast(await invoke("read_book_delete_file", { key: x.key, othersOk: true })); return true; } catch (e2) { toast(String(e2), true); return false; }
    }
    if (others.length || !/still reads/.test(String(e))) toast(String(e), true);
    return false;
  }
}

/** The reader goes over everything: the panel (a modal, drawn above all
 *  else) closes first and comes back, up to date, when the reader closes. */
function read(key, chapter, c) {
  if (dlg) dlg.close();
  openReader(key, chapter, () => { openSeries({ key, card: c }); window.dispatchEvent(new Event("chillarr:refresh")); });
}

/** The page behind the panel changed: it's drawn again once, when the panel
 *  closes, rather than on every click in it (each redraw asked for the whole
 *  reading list again). */
let staleBehind = false;

function refreshIfOpen(key, c0) {
  if (dlg && dlg.open) { showFollowed(key, ++panelSeq, c0); staleBehind = true; return; }
  window.dispatchEvent(new Event("chillarr:refresh"));
}

/** Another copy of a series you follow (another site or translation); what you've read stays. */
function changeCopy(c, f, body) {
  const mine = panelSeq;
  if (body.querySelector(".read-change")) return; // already open
  const list = el("section", { class: "read-sources read-change" });
  const options = el("div", { class: "read-options read-change" });
  const state = { lang: f.lang || getPref("read-lang") || "en", copy: "choose", onChange: () => getCopies(c, state, list, mine) };
  pickers(options, state);
  body.prepend(options, list);
  list.scrollIntoView({ behavior: "smooth", block: "start" });
  getCopies(c, state, list, mine);
}

/** Series being let go right now: a second press while one runs does nothing. */
const unfollowing = new Set();

async function unfollow(f, btn) {
  if (unfollowing.has(f.key)) return;
  if (!(await ask(`Stop following ${f.title}?`, "New chapters stop downloading.", [], "Stop following", "Cancel"))) return;
  // Asked as a choice: closing it (Esc) stops nothing, rather than reading as "Keep them".
  const pick = await choose("Delete its chapters too?", "Keep them to read later, or free the space now.", [], "Delete them", "Keep them");
  if (pick == null) return;
  unfollowing.add(f.key);
  // It takes a few seconds with many chapters: the panel goes at once and says so,
  // instead of sitting there looking like nothing happened.
  if (btn) { btn.disabled = true; btn.textContent = "Stopping..."; }
  if (dlg) dlg.close();
  toast(`Stopping ${f.title}...`);
  try {
    toast(await invoke("read_unfollow", { key: f.key, delete: pick === "yes" }));
    followedByAnilist.delete(f.anilist);
  } catch (e) {
    toast(String(e), true);
  } finally {
    unfollowing.delete(f.key);
    window.dispatchEvent(new Event("chillarr:refresh"));
  }
}

/* ---------- the book panel ---------- */

const isComic = (c) => c.kind === "comic";
// A book and the comic of the same story are two things to get.
const sameBook = (c, x) => isComic(c) === !!x.comic && ((c.olid && x.olid === c.olid) || (c.anilist && x.anilist === c.anilist) || (x.title || "").toLowerCase() === (c.title || "").toLowerCase());
const bookMeta = (c) => ({ title: c.title, author: c.author ?? null, cover: c.cover ?? null, description: c.description ?? null, olid: c.olid ?? null, anilist: c.anilist ?? null, comic: isComic(c) });

/** A book or light novel: read it if it's here, else find a copy. */
export async function openBookPanel(c, mine = null) {
  const my = ++panelSeq;
  delOk.clear();
  clearTimeout(poll);
  if (!mine) {
    myBooks = await invoke("read_books").catch(() => myBooks);
    if (my !== panelSeq) return;
    mine = myBooks.find((x) => sameBook(c, x)) || null;
  }
  const shown = { ...c, chapters: null, status: null, genres: c.genres || [] };
  const { actions, body, hideOn } = shell(shown);
  if (c.author) {
    const by = el("button", { class: "maker-link", title: `All books by ${c.author}`, onclick: () => {
      if (dlg) dlg.close();
      location.hash = `#r-author/${encodeURIComponent(c.author)}`;
    } }, c.author);
    actions.parentElement.querySelector(".meta").prepend(by, el("span", { class: "sep" }));
  }
  if (mine) {
    if (mine.ready) {
      const go = (fromStart) => () => { if (dlg) dlg.close(); openBook(mine.key, () => openBookPanel(c), fromStart); };
      const done = mine.finished || mine.read >= 1;
      // Finished (90% in): Read again starts over; Resume still goes on from where you stopped.
      actions.append(el("button", { class: "btn play", onclick: go(done) }, icon(ICON.play),
        done ? "Read again" : mine.read > 0 ? `Resume (${Math.round(mine.read * 100)}%)` : "Read"));
      if (done && mine.read < 0.995) actions.append(el("button", { class: "btn soft", onclick: go(false) }, `Resume (${Math.round(mine.read * 100)}%)`));
      if (await mayDelete(mine.key)) actions.append(el("button", { class: "btn soft", onclick: async () => { if (await deleteBookFile(mine)) { window.dispatchEvent(new Event("chillarr:refresh")); openBookPanel(c); } } }, icon(TRASH), "Delete file"));
    } else if (mine.not_downloaded) {
      // Its file was deleted: still in the list, downloaded again from here.
      actions.append(el("span", { class: "status-pill", text: "Not downloaded" }));
      const again = el("button", { class: "btn primary" }, icon(ICON.download), "Find copies");
      actions.append(again);
      const list = el("section", { class: "read-sources" });
      body.prepend(list);
      again.addEventListener("click", () => bookCopies(c, list, again, my));
    } else {
      actions.append(el("span", { class: "status-pill", text: mine.problem || (mine.progress != null ? `Downloading ${Math.round(mine.progress * 100)}%` : "Starting the download") }));
      poll = setTimeout(() => { if (my === panelSeq && dlg && dlg.open) openBookPanel(c); }, 4000);
    }
    actions.append(el("button", { class: "btn soft", onclick: () => removeBook(mine) }, "Remove"), ...bookNotify(c));
    hideOn();
    if (mine.problem) { const list = el("section", { class: "read-sources" }); body.append(list); bookCopies(c, list, el("button", {}, "x"), my); }
    return;
  }
  const find = el("button", { class: "btn primary" }, icon(ICON.download), "Find copies");
  actions.append(find, listButton(c), ...bookNotify(c));
  hideOn();
  const list = el("section", { class: "read-sources" });
  body.prepend(list);
  find.addEventListener("click", () => bookCopies(c, list, find, my));
}

/** Notify me for a light novel's new volumes, or a book author's new books (notify.js). */
function bookNotify(c) {
  if (!c.anilist && !c.author) return [];
  return notifyButton({ book: { title: c.title, author: c.author || null, cover: c.cover || null, anilist: c.anilist || null } }, c.anilist ? "Notify me of new volumes" : "Notify me of new books");
}

async function bookCopies(c, list, btn, my) {
  btn.disabled = true;
  list.replaceChildren(el("p", { class: "note read-wait", text: "Searching the download sites..." }));
  let found;
  try { found = await invoke("read_book_copies", { meta: bookMeta(c) }); }
  catch (e) { if (my !== panelSeq) return; list.replaceChildren(note(String(e))); btn.disabled = false; return; }
  if (my !== panelSeq) return;
  btn.remove();
  if (!found.length) { list.replaceChildren(note(`No copy of this ${isComic(c) ? "comic" : "book"} was found. Try again another day, or search with another title.`)); return; }
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
        el("div", { class: "read-source-text" }, el("strong", { text: (x.format || (isComic(c) ? "comic" : "ebook")).toUpperCase() }), i === 0 ? el("span", { class: "read-best", text: "Best" }) : null,
          el("small", { text: `${mb(x.size)} · ${x.seeders} sharing · ${x.indexer}` }), el("small", { class: "read-alt", text: x.title })), go);
    }));
}

async function removeBook(x) {
  if (!(await ask(`Remove ${x.title}?`, `The ${x.comic ? "comic" : "book"}'s file is deleted from your PC. You can download it again.`, [], "Remove", "Cancel"))) return;
  try { toast(await invoke("read_book_remove", { key: x.key })); if (dlg) dlg.close(); window.dispatchEvent(new Event("chillarr:refresh")); }
  catch (e) { toast(String(e), true); }
}
