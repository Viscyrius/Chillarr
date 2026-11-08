// Movies, Series and Anime filters, the same look and options as reading's
// (#40, filterpanel.js): a Filters button with a count folding open a panel
// under the header, Order beside it. Genres and themes in or out, status and
// rating up top; years, length, language, country, studio or network, age
// rating, the anime parts and 18+ (anime only) under More filters. Kept per
// profile and per type; results load as you scroll.
import { el, getPref, icon, ICON, invoke, setPref } from "./api.js";
import { currentSeq, infiniteGrid, stopBrowse, swapIn } from "./browse.js";
import { ADULT_CHOICES, chipKit, clearButton, filtersButton, moreButton, orderSelect } from "./filterpanel.js";
import { goBack } from "./place.js";

const KINDS = [["movie", "Movies"], ["series", "Series"], ["anime", "Anime"]];
const SORTS = [["popular", "Most popular"], ["top", "Top rated"], ["new", "Newest"], ["title", "A to Z"]];
const RATINGS = [[null, "Any"], [6, "6+"], [7, "7+"], [8, "8+"], [9, "9+"]];
const FORMATS = [["TV", "TV"], ["MOVIE", "Movie"], ["OVA", "OVA"], ["ONA", "ONA"], ["SPECIAL", "Special"], ["TV_SHORT", "Short"]];
const STATUSES = [[null, "Any"], ["RELEASING", "Airing"], ["FINISHED", "Finished"], ["NOT_YET_RELEASED", "Coming soon"]];
const SEASONS = [[null, "Any"], ["WINTER", "Winter"], ["SPRING", "Spring"], ["SUMMER", "Summer"], ["FALL", "Fall"]];
const LENGTHS = [[null, null, "Any"], [null, 13, "Up to 13"], [14, 26, "14 to 26"], [27, null, "27 or more"]];
// AniList tag names, checked against its tag list. The first 20 are the Themes row.
const POPULAR_TAGS = ["Isekai", "Shounen", "Seinen", "Shoujo", "Josei", "Female Harem", "Male Harem", "School", "Super Power", "Magic", "Martial Arts", "Swordplay",
  "Military", "Demons", "Reincarnation", "Time Loop", "Revenge", "Tragedy", "Iyashikei", "Cute Girls Doing Cute Things", "Anti-Hero", "Female Protagonist",
  "Primarily Adult Cast", "Love Triangle", "Post-Apocalyptic", "Dystopian", "Space", "Cyberpunk", "Video Games", "Death Game", "Battle Royale", "Mythology",
  "Historical", "Urban Fantasy", "Detective", "Crime", "Mafia", "Yuri", "Boys' Love", "Found Family", "Cultivation", "Dungeon", "Work", "Food", "Band", "Idol",
  "Parody", "Vampire", "Zombie", "Ghost", "Survival", "Gore"];

// Original language (ISO codes, the same for movies and series). For series
// Korean, Japanese, Chinese, Thai and Turkish also match the network's country.
const LANGS = [["en", "English"], ["ko", "Korean"], ["ja", "Japanese"], ["zh", "Chinese"], ["hi", "Hindi"], ["es", "Spanish"], ["fr", "French"],
  ["de", "German"], ["it", "Italian"], ["tr", "Turkish"], ["th", "Thai"], ["pt", "Portuguese"], ["sv", "Swedish"], ["da", "Danish"], ["no", "Norwegian"]];
// Movies.
const CERTS = ["G", "PG", "PG-13", "R", "NC-17"];
const STAGES = [[null, "Any"], ["released", "Released"], ["cinema", "In cinemas"], ["soon", "Coming soon"]];
const MOVIE_LENGTHS = [[null, null, "Any"], [null, 89, "Under 90 min"], [90, 120, "90 min to 2 hours"], [121, 150, "2 to 2.5 hours"], [151, null, "Longer"]];
// Series (TVmaze).
const SHOW_STATUSES = [[null, "Any"], ["Running", "Running"], ["Ended", "Ended"], ["In Development", "Coming soon"]];
const SHOW_LENGTHS = [[null, null, "Any"], [null, 30, "Up to 30 min"], [31, 50, "31 to 50 min"], [51, null, "Over 50 min"]];
const SHOW_TYPES = [["Scripted", "Drama and comedy"], ["Animation", "Animation"], ["Documentary", "Documentary"], ["Reality", "Reality"], ["Talk Show", "Talk show"],
  ["Game Show", "Game show"], ["Variety", "Variety"], ["Sports", "Sports"], ["News", "News"]];
const COUNTRIES = [["US", "United States"], ["GB", "United Kingdom"], ["KR", "South Korea"], ["JP", "Japan"], ["CA", "Canada"], ["AU", "Australia"],
  ["FR", "France"], ["DE", "Germany"], ["ES", "Spain"], ["IN", "India"], ["CN", "China"]];

const ANIME_ONLY = { tags: [], not_tags: [], formats: [], season: null, episodes_min: null, episodes_max: null, adult: "hide" };
const CATALOG_ONLY = { countries: [], runtime_min: null, runtime_max: null, certs: [], studios: [], show_types: [], not_show_types: [] };
const BLANK = { genres: [], not_genres: [], year_from: null, year_to: null, min_rating: null, sort: "popular", owned: false,
  status: null, languages: [], not_languages: [], ...ANIME_ONLY, ...CATALOG_ONLY };

let tagList = null;
const studioLists = {};
const studiosOf = (kind) => (kind === "anime" ? [] : studioLists[kind] || invoke("media_filter_studios", { kind }).then((v) => (studioLists[kind] = v)).catch(() => []));

/** Only what differs from a blank filter goes in the link or the saved copy. */
function trimmed(f) {
  const out = {};
  for (const [k, v] of Object.entries(f)) {
    if (k === "kind" || v === null || v === false || (Array.isArray(v) && !v.length) || (k === "sort" && v === "popular") || (k === "adult" && v === "hide")) continue;
    out[k] = v;
  }
  return out;
}
const linkOf = (f) => `#filter/${f.kind}/${encodeURIComponent(JSON.stringify(trimmed(f)))}`;
const prefOf = (kind) => `filter-${kind}`;

function filterCount(f) {
  return f.genres.length + f.not_genres.length + f.tags.length + f.not_tags.length + f.languages.length + f.not_languages.length
    + !!f.status + !!f.min_rating + !!(f.year_from || f.year_to) + !!(f.runtime_min || f.runtime_max) + !!(f.episodes_min || f.episodes_max)
    + f.formats.length + !!f.season + f.certs.length + f.studios.length + f.show_types.length + f.not_show_types.length + f.countries.length
    + !!f.owned + (f.adult && f.adult !== "hide" ? 1 : 0);
}

// The page's pending reload; one page at a time.
let reloadTimer = null;

export async function renderFilter(view, kind, start, mode = null, want = 0) {
  stopBrowse();
  clearTimeout(reloadTimer);
  const drawSeq = currentSeq();
  kind = KINDS.some(([k]) => k === kind) ? kind : "movie";
  // A link that names filters (a genre link, Back) sets them; a bare one opens
  // with this profile's last filters for this type.
  const named = start && Object.keys(start).length;
  const saved = (() => { try { return JSON.parse(getPref(prefOf(kind)) || "{}"); } catch (e) { return {}; } })();
  const f = { ...BLANK, ...(named ? start : saved), kind };
  // Links from before genres could be combined carry one "genre".
  if (f.genre) { f.genres = [...new Set([...(f.genres || []), f.genre])]; delete f.genre; }
  // 18+ is for anime: TMDB's adult flag isn't in the movie catalog, and TVmaze has none.
  if (kind !== "anime") f.adult = "hide";

  const [genres, studios] = await Promise.all([invoke("media_filter_genres", { kind }).catch(() => []), studiosOf(kind)]);
  // Another page was opened while these loaded.
  if (drawSeq !== currentSeq()) return;
  const page = el("div", { class: "fl fl-new enter" });
  const panel = el("div", { class: "fl-bar read-fl" });
  const results = el("div", { class: "fl-results" });
  const back = el("button", { class: "back-link", onclick: () => goBack() }, icon(ICON.left), "Back");
  const { button: toggle, count: showCount } = filtersButton(panel, "media-filters-open");
  if (named) { panel.classList.remove("shut"); toggle.setAttribute("aria-expanded", "true"); }
  const order = orderSelect(SORTS, f.sort, (v) => change((x) => (x.sort = v), { now: true }));
  const title = (KINDS.find(([k]) => k === kind) || KINDS[0])[1];
  page.append(el("div", { class: "lib-head" }, el("div", {}, back, el("h1", { text: title })), el("div", { class: "read-filters" }, toggle, order)), panel, results);
  // Coming back (browse.js swapIn), the old page stays up until this one has as many results as when you left.
  if (!mode) view.replaceChildren(page);

  let moreOpen = false;
  /** Shows the change on the controls now and reloads the results a moment later (several clicks, one load). */
  function change(mut, { now = false } = {}) {
    mut(f);
    // The entry's own state stays: it is how Back finds where you were (place.js).
    history.replaceState(history.state, "", linkOf(f));
    setPref(prefOf(kind), JSON.stringify(trimmed(f)));
    drawPanel();
    clearTimeout(reloadTimer);
    // Not once you've left: its stopBrowse() would stop the new page's loading.
    reloadTimer = setTimeout(() => { if (page.isConnected) loadResults(); }, now ? 0 : 250);
  }
  const { triChip, oneOf, anyOf, field, row, years } = chipKit(f, change);

  function loadResults(upTo = 0) {
    stopBrowse();
    const seq = currentSeq();
    const snapshot = { ...f };
    const g = infiniteGrid(seq, (p) => invoke("media_filter", { filter: snapshot, page: p }),
      (n) => (n ? `That's all ${n}.` : f.owned ? "Nothing in your library matches." : "Nothing matches. Try taking a filter off."));
    results.replaceChildren(...g.nodes);
    return g.fill(upTo);
  }

  /** Off while Only what I have is on: the library has none of this data. */
  const notOwned = (parts, what) => (f.owned ? [el("p", { class: "fl-hint", text: `${what} only work without Only what I have.` }), el("div", { class: "fl-off", inert: true }, ...parts)] : parts);
  const lengthField = (label, options, minKey, maxKey) => field(label,
    oneOf(options, ([a, b]) => f[minKey] === a && f[maxKey] === b, (x, [a, b]) => { x[minKey] = a; x[maxKey] = b; }));

  function tagFinder() {
    const search = el("input", { class: "fl-search", type: "search", placeholder: "Find another tag", list: "fl-tags", autocomplete: "off" });
    const options = el("datalist", { id: "fl-tags" });
    const fill = () => options.replaceChildren(...(tagList || []).map(([name]) => el("option", { value: name })));
    if (tagList) fill();
    else invoke("media_anime_tags").then((t) => { tagList = t; if (options.isConnected) fill(); }).catch(() => { search.placeholder = "More tags need AniList, which didn't answer"; });
    search.addEventListener("change", () => {
      const hit = (tagList || []).find(([name]) => name.toLowerCase() === search.value.trim().toLowerCase());
      if (hit && !f.tags.includes(hit[0])) change((x) => (x.tags = [...x.tags, hit[0]]));
      search.value = "";
    });
    return el("div", { class: "fl-tag-find" }, search, options);
  }

  function seasonField() {
    const y = new Date().getFullYear();
    const oneYear = f.year_from && f.year_from === f.year_to ? f.year_from : null;
    const year = el("select", { class: "filter-select", "aria-label": "Season year" },
      el("option", { value: "", text: "Any year" }),
      Array.from({ length: y + 2 - 1960 }, (_, i) => y + 1 - i).map((v) => el("option", { value: String(v), text: String(v), selected: oneYear === v })));
    year.addEventListener("change", () => change((x) => { const v = year.value ? +year.value : null; x.year_from = v; x.year_to = v; }, { now: true }));
    return field("Season",
      el("p", { class: "fl-hint", text: "When it first aired: Winter (January), Spring (April), Summer (July) or Fall (October)." }),
      el("div", { class: "fl-season" }, oneOf(SEASONS, ([v]) => f.season === v, (x, [v]) => (x.season = v)), el("label", { class: "fl-inline" }, el("small", { text: "Year" }), year)));
  }

  function drawPanel() {
    const n = filterCount(f);
    showCount(n);
    const anime = kind === "anime", series = kind === "series";
    const gs = anime && f.adult !== "hide" ? [...genres, "Hentai"] : genres;
    const owned = el("input", { type: "checkbox", class: "switch", checked: f.owned, "aria-label": "Only what I have" });
    owned.addEventListener("change", () => change((x) => (x.owned = owned.checked), { now: true }));
    const parts = [
      field("Genres", gs.length ? el("div", { class: "chips" }, gs.map((g) => triChip(g, "genres", "not_genres"))) : el("p", { class: "note", text: "Genres show up once the catalog has filled in." })),
      // Themes: AniList tags for anime; for series TVmaze's kind of show is the closest there is.
      anime ? field("Themes", ...notOwned([el("div", { class: "chips" }, POPULAR_TAGS.slice(0, 20).map((t) => triChip(t, "tags", "not_tags")))], "Themes"))
        : series ? field("Themes", ...notOwned([el("div", { class: "chips" }, SHOW_TYPES.map(([v, t]) => triChip(v, "show_types", "not_show_types", t)))], "Themes")) : null,
      row(
        field("Status", ...(anime ? notOwned([oneOf(STATUSES, ([v]) => f.status === v, (x, [v]) => (x.status = v))], "Status")
          : notOwned([oneOf(series ? SHOW_STATUSES : STAGES, ([v]) => f.status === v, (x, [v]) => (x.status = v))], "Status"))),
        field("Rating", oneOf(RATINGS, ([v]) => f.min_rating === v, (x, [v]) => (x.min_rating = v)))),
      moreButton(moreOpen, () => { moreOpen = !moreOpen; drawPanel(); }),
    ];
    if (moreOpen) {
      if (anime) {
        parts.push(...notOwned([
          field("More themes", el("p", { class: "fl-hint", text: "Click once to show only these, twice to leave them out." }),
            el("div", { class: "chips" }, [...new Set([...POPULAR_TAGS.slice(20), ...f.tags, ...f.not_tags])].filter((t) => !POPULAR_TAGS.slice(0, 20).includes(t)).map((t) => triChip(t, "tags", "not_tags"))), tagFinder()),
          row(field("Format", anyOf(FORMATS, "formats")), lengthField("Episodes", LENGTHS, "episodes_min", "episodes_max")),
          seasonField()], "Themes, format, episodes and season"));
      } else {
        parts.push(field("Language", ...notOwned([el("p", { class: "fl-hint", text: series ? "The language it was made in. Korean, Japanese and Chinese also find shows from those countries." : "The language it was made in." }),
          el("div", { class: "chips" }, LANGS.map(([v, t]) => triChip(v, "languages", "not_languages", t)))], "Languages")));
        const own = series
          ? [lengthField("Episode length", SHOW_LENGTHS, "runtime_min", "runtime_max"),
            field("Network or streaming", el("p", { class: "fl-hint", text: "Where it first aired." }), [...new Set([...studios, ...f.studios])].length ? anyOf([...new Set([...studios, ...f.studios])].map((v) => [v, v]), "studios") : el("p", { class: "note", text: "These show up once the catalog has filled in." })),
            field("Country", el("p", { class: "fl-hint", text: "Where the network or streaming service is based." }), anyOf(COUNTRIES, "countries"))]
          : [row(lengthField("Length", MOVIE_LENGTHS, "runtime_min", "runtime_max"),
            field("Age rating", el("p", { class: "fl-hint", text: "US ratings. G is for everyone, R is for adults." }), anyOf(CERTS.map((c) => [c, c]), "certs"))),
            field("Studio", el("p", { class: "fl-hint", text: "The studios with the most movies here." }), [...new Set([...studios, ...f.studios])].length ? anyOf([...new Set([...studios, ...f.studios])].map((v) => [v, v]), "studios") : el("p", { class: "note", text: "These show up once the catalog has filled in." }))];
        parts.push(...notOwned(own, series ? "Length, network and country" : "Length, age rating and studio"));
      }
      parts.push(row(field("Years", years()),
        field("Only what I have", el("label", { class: "fl-inline fl-owned" }, owned, el("small", { text: "Just the titles in your library" })))));
      if (anime) {
        parts.push(field("18+", ...notOwned([oneOf(ADULT_CHOICES, (o) => f.adult === o[0], (x, o) => (x.adult = o[0])),
          el("small", { class: "read-fl-hint", text: "Off until you pick Include or Only 18+." })], "18+")));
      }
    }
    if (n) parts.push(clearButton(() => change((x) => Object.assign(x, { ...BLANK, sort: x.sort }), { now: true })));
    panel.replaceChildren(...parts.filter(Boolean));
  }

  drawPanel();
  if (!mode) { loadResults(); return; }
  const loaded = loadResults(want);
  const seq = currentSeq();
  await loaded;
  // The grid carries on loading by itself once it's on screen.
  if (seq === currentSeq()) swapIn(view, [page], mode);
}
