// The Filters page: type, genres (in or out), language, years, rating and order
// up top; years by hand and each type's own parts under "More filters" (anime:
// tags, format, airing, season, episodes; movies: out now, age rating, length,
// studio; series: airing, length, kind, network, country). Every change shows
// at once; results load as you scroll.
import { el, icon, ICON, invoke } from "./api.js";
import { currentSeq, infiniteGrid, stopBrowse, swapIn } from "./browse.js";
import { goBack } from "./place.js";

const KINDS = [["movie", "Movies"], ["series", "Series"], ["anime", "Anime"]];
const SORTS = [["popular", "Most popular"], ["top", "Highest rated"], ["new", "Newest"], ["title", "A to Z"]];
const RATINGS = [[null, "Any"], [6, "6+"], [7, "7+"], [8, "8+"], [9, "9+"]];
const FORMATS = [["TV", "TV"], ["MOVIE", "Movie"], ["OVA", "OVA"], ["ONA", "ONA"], ["SPECIAL", "Special"], ["TV_SHORT", "Short"]];
const STATUSES = [[null, "Any"], ["RELEASING", "Airing now"], ["FINISHED", "Finished"], ["NOT_YET_RELEASED", "Coming soon"]];
const SEASONS = [[null, "Any"], ["WINTER", "Winter"], ["SPRING", "Spring"], ["SUMMER", "Summer"], ["FALL", "Fall"]];
const LENGTHS = [[null, null, "Any"], [null, 13, "Up to 13"], [14, 26, "14 to 26"], [27, null, "27 or more"]];
// AniList tag names, checked against its tag list.
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
const STAGES = [[null, "Any"], ["released", "Out to watch"], ["cinema", "In cinemas"], ["soon", "Coming soon"]];
const MOVIE_LENGTHS = [[null, null, "Any"], [null, 89, "Under 90 min"], [90, 120, "90 min to 2 hours"], [121, 150, "2 to 2.5 hours"], [151, null, "Longer"]];
// Series (TVmaze).
const SHOW_STATUSES = [[null, "Any"], ["Running", "Still running"], ["Ended", "Ended"], ["In Development", "Coming soon"]];
const SHOW_LENGTHS = [[null, null, "Any"], [null, 30, "Up to 30 min"], [31, 50, "31 to 50 min"], [51, null, "Over 50 min"]];
const SHOW_TYPES = [["Scripted", "Drama and comedy"], ["Animation", "Animation"], ["Documentary", "Documentary"], ["Reality", "Reality"], ["Talk Show", "Talk show"],
  ["Game Show", "Game show"], ["Variety", "Variety"], ["Sports", "Sports"], ["News", "News"]];
const COUNTRIES = [["US", "United States"], ["GB", "United Kingdom"], ["KR", "South Korea"], ["JP", "Japan"], ["CA", "Canada"], ["AU", "Australia"],
  ["FR", "France"], ["DE", "Germany"], ["ES", "Spain"], ["IN", "India"], ["CN", "China"]];

const ANIME_ONLY = { tags: [], not_tags: [], formats: [], season: null, episodes_min: null, episodes_max: null };
const CATALOG_ONLY = { countries: [], runtime_min: null, runtime_max: null, certs: [], studios: [], show_types: [], not_show_types: [] };
const BLANK = { genres: [], not_genres: [], year_from: null, year_to: null, min_rating: null, sort: "popular", owned: false,
  status: null, languages: [], not_languages: [], ...ANIME_ONLY, ...CATALOG_ONLY };
const labelOf = (list, v) => { const hit = list.find((o) => o[0] === v); return hit ? hit[hit.length - 1] : v; };

function yearPresets() {
  const y = new Date().getFullYear();
  const decade = (d) => [d, Math.min(d + 9, y + 1), `${d}s`];
  return [[null, null, "Any"], [y, y, "This year"], [y - 4, y, "Last 5 years"], decade(2020), decade(2010), decade(2000), decade(1990), [null, 1989, "Older"]];
}

let advancedOpen = (() => { try { return localStorage.getItem("chillarr-filter-adv") === "1"; } catch (e) { return false; } })();
let tagList = null;
const studioLists = {};
const studiosOf = (kind) => (kind === "anime" ? [] : studioLists[kind] || invoke("media_filter_studios", { kind }).then((v) => (studioLists[kind] = v)).catch(() => []));

/** Only what differs from a blank filter goes in the link. */
function linkOf(f) {
  const out = {};
  for (const [k, v] of Object.entries(f)) {
    if (k === "kind" || v === null || v === false || (Array.isArray(v) && !v.length) || (k === "sort" && v === "popular")) continue;
    out[k] = v;
  }
  return `#filter/${f.kind}/${encodeURIComponent(JSON.stringify(out))}`;
}

export async function renderFilter(view, kind, start, mode = null, want = 0) {
  stopBrowse();
  const f = { ...BLANK, ...start };
  // Links from before genres could be combined carry one "genre".
  if (f.genre) { f.genres = [...new Set([...(f.genres || []), f.genre])]; delete f.genre; }
  f.kind = KINDS.some(([k]) => k === kind) ? kind : "movie";

  let [genres, studios] = await Promise.all([invoke("media_filter_genres", { kind: f.kind }).catch(() => []), studiosOf(f.kind)]);
  const page = el("div", { class: "fl enter" });
  const bar = el("div", { class: "fl-bar" });
  const results = el("div", { class: "fl-results" });
  const back = el("button", { class: "back-link", onclick: () => goBack() }, icon(ICON.left), "Back");
  page.append(el("div", { class: "lib-head" }, el("div", {}, back, el("h1", { text: "Filters" }))), bar, results);
  // Coming back (browse.js swapIn), the old page stays up until this one has as many results as when you left.
  if (!mode) view.replaceChildren(page);

  let reloadTimer = null;
  /** Shows the change on the controls now and reloads the results a moment later (several clicks, one load). */
  async function change(mut, { now = false } = {}) {
    const kindBefore = f.kind;
    mut(f);
    if (f.kind !== kindBefore) {
      // Genres, airing and the More filters parts differ per type. Language
      // means the same for movies and series, so it stays between those two.
      Object.assign(f, { genres: [], not_genres: [], status: null, ...ANIME_ONLY, ...CATALOG_ONLY });
      if (f.kind === "anime" || kindBefore === "anime") Object.assign(f, { languages: [], not_languages: [] });
      [genres, studios] = await Promise.all([invoke("media_filter_genres", { kind: f.kind }).catch(() => []), studiosOf(f.kind)]);
    }
    // The entry's own state stays: it is how Back finds where you were (place.js).
    history.replaceState(history.state, "", linkOf(f));
    drawBar();
    clearTimeout(reloadTimer);
    reloadTimer = setTimeout(loadResults, now ? 0 : 250);
  }

  function loadResults(upTo = 0) {
    stopBrowse();
    const seq = currentSeq();
    const snapshot = { ...f };
    const g = infiniteGrid(seq, (p) => invoke("media_filter", { filter: snapshot, page: p }),
      (n) => (n ? `That's all ${n}.` : f.owned ? "Nothing in your library matches." : "Nothing matches. Try taking a filter off."));
    results.replaceChildren(...g.nodes);
    return g.fill(upTo);
  }

  const toggle = (list, v) => (list.includes(v) ? list.filter((x) => x !== v) : [...list, v]);
  // Off -> in -> out -> off.
  function cycle(inKey, outKey, v) {
    change((x) => {
      if (x[inKey].includes(v)) { x[inKey] = x[inKey].filter((g) => g !== v); x[outKey] = [...x[outKey], v]; }
      else if (x[outKey].includes(v)) x[outKey] = x[outKey].filter((g) => g !== v);
      else x[inKey] = [...x[inKey], v];
    });
  }
  const triChip = (v, inKey, outKey, label = v) => {
    const on = f[inKey].includes(v), out = f[outKey].includes(v);
    return el("button", { class: `chip${on ? " on" : ""}${out ? " out" : ""}`, "aria-pressed": on ? "true" : out ? "mixed" : "false",
      title: on ? "Showing only these. Click to leave them out." : out ? "Leaving these out. Click to clear." : "Click to show only these",
      onclick: () => cycle(inKey, outKey, v) }, out ? icon("M5 12h14") : on ? icon(ICON.check) : null, label);
  };
  // Any of these: click to add, again to take off.
  const anyChips = (options, key) => el("div", { class: "chips" }, options.map(([v, t]) =>
    el("button", { class: `chip${f[key].includes(v) ? " on" : ""}`, "aria-pressed": String(f[key].includes(v)), onclick: () => change((x) => (x[key] = toggle(x[key], v))) }, t)));
  const oneOf = (options, isOn, pick) => el("div", { class: "chips" }, options.map((o) =>
    el("button", { class: `chip${isOn(o) ? " on" : ""}`, onclick: () => change((x) => pick(x, o)) }, o[o.length - 1])));
  const field = (label, ...kids) => el("div", { class: "fl-field" }, el("h3", { text: label }), ...kids);
  const yearSelect = (label, key) => {
    const y = new Date().getFullYear();
    const s = el("select", { class: "filter-select", "aria-label": label },
      el("option", { value: "", text: "Any" }), Array.from({ length: y + 2 - 1920 }, (_, i) => y + 1 - i).map((v) => el("option", { value: String(v), text: String(v), selected: f[key] === v })));
    s.addEventListener("change", () => change((x) => {
      x[key] = s.value ? +s.value : null;
      // Keep From before To.
      if (x.year_from && x.year_to && x.year_from > x.year_to) { if (key === "year_from") x.year_to = x.year_from; else x.year_from = x.year_to; }
    }, { now: true }));
    return el("label", { class: "fl-inline" }, el("small", { text: label }), s);
  };

  /** What's on, each with an x to take it off. */
  function pills() {
    const out = [];
    const pill = (text, off, cls = "") => out.push(el("button", { class: `fl-pill${cls}`, title: "Take this off", onclick: () => change(off) }, text, icon("M6 6l12 12M18 6L6 18")));
    f.genres.forEach((g) => pill(g, (x) => (x.genres = x.genres.filter((v) => v !== g))));
    f.not_genres.forEach((g) => pill(`Not ${g}`, (x) => (x.not_genres = x.not_genres.filter((v) => v !== g)), " out"));
    f.tags.forEach((g) => pill(g, (x) => (x.tags = x.tags.filter((v) => v !== g))));
    f.not_tags.forEach((g) => pill(`Not ${g}`, (x) => (x.not_tags = x.not_tags.filter((v) => v !== g)), " out"));
    f.languages.forEach((g) => pill(labelOf(LANGS, g), (x) => (x.languages = x.languages.filter((v) => v !== g))));
    f.not_languages.forEach((g) => pill(`Not ${labelOf(LANGS, g)}`, (x) => (x.not_languages = x.not_languages.filter((v) => v !== g)), " out"));
    // A season with one year reads as one pill: "Fall 2025".
    const oneYear = f.year_from && f.year_from === f.year_to;
    const seasonName = f.season ? SEASONS.find(([k]) => k === f.season)?.[1] || f.season : null;
    if (seasonName && oneYear) pill(`${seasonName} ${f.year_from}`, (x) => { x.season = null; x.year_from = null; x.year_to = null; });
    else if (f.year_from || f.year_to) {
      const t = f.year_from && f.year_to ? (f.year_from === f.year_to ? `${f.year_from}` : `${f.year_from} to ${f.year_to}`) : f.year_from ? `From ${f.year_from}` : `Up to ${f.year_to}`;
      pill(t, (x) => { x.year_from = null; x.year_to = null; });
    }
    if (f.min_rating) pill(`Rated ${f.min_rating}+`, (x) => (x.min_rating = null));
    f.formats.forEach((v) => pill(FORMATS.find(([k]) => k === v)?.[1] || v, (x) => (x.formats = x.formats.filter((y) => y !== v))));
    if (f.status) pill(labelOf(f.kind === "movie" ? STAGES : f.kind === "series" ? SHOW_STATUSES : STATUSES, f.status), (x) => (x.status = null));
    f.certs.forEach((v) => pill(`Rated ${v}`, (x) => (x.certs = x.certs.filter((y) => y !== v))));
    if (f.runtime_min || f.runtime_max) {
      const hit = (f.kind === "series" ? SHOW_LENGTHS : MOVIE_LENGTHS).find(([a, b]) => a === f.runtime_min && b === f.runtime_max);
      pill(hit ? hit[2] : "Length", (x) => { x.runtime_min = null; x.runtime_max = null; });
    }
    f.show_types.forEach((v) => pill(labelOf(SHOW_TYPES, v), (x) => (x.show_types = x.show_types.filter((y) => y !== v))));
    f.not_show_types.forEach((v) => pill(`Not ${labelOf(SHOW_TYPES, v)}`, (x) => (x.not_show_types = x.not_show_types.filter((y) => y !== v)), " out"));
    f.studios.forEach((v) => pill(v, (x) => (x.studios = x.studios.filter((y) => y !== v))));
    f.countries.forEach((v) => pill(labelOf(COUNTRIES, v), (x) => (x.countries = x.countries.filter((y) => y !== v))));
    if (seasonName && !oneYear) pill(`${seasonName}, any year`, (x) => (x.season = null));
    if (f.episodes_min || f.episodes_max) pill(LENGTHS.find(([a, b]) => a === f.episodes_min && b === f.episodes_max)?.[2] || "Length", (x) => { x.episodes_min = null; x.episodes_max = null; });
    if (f.owned) pill("Only what I have", (x) => (x.owned = false));
    return out;
  }

  /** Season, with the year it goes with: AniList files every anime under the
   *  season it started in. Alone it means that season in any year. */
  function seasonField() {
    const y = new Date().getFullYear();
    const oneYear = f.year_from && f.year_from === f.year_to ? f.year_from : null;
    const year = el("select", { class: "filter-select", "aria-label": "Season year" },
      el("option", { value: "", text: "Any year" }),
      Array.from({ length: y + 2 - 1960 }, (_, i) => y + 1 - i).map((v) => el("option", { value: String(v), text: String(v), selected: oneYear === v })));
    year.addEventListener("change", () => change((x) => { const v = year.value ? +year.value : null; x.year_from = v; x.year_to = v; }, { now: true }));
    const name = f.season ? SEASONS.find(([k]) => k === f.season)?.[1] : null;
    const now = name ? (oneYear ? `Showing anime that started in ${name} ${oneYear}.` : `Showing ${name} anime from every year. Pick a year for one season.`) : null;
    return field("Season",
      el("p", { class: "fl-hint", text: "When it first aired. New anime start four times a year: Winter (January), Spring (April), Summer (July) and Fall (October)." }),
      el("div", { class: "fl-season" }, oneOf(SEASONS, ([v]) => f.season === v, (x, [v]) => (x.season = v)), el("label", { class: "fl-inline" }, el("small", { text: "Year" }), year)),
      now ? el("p", { class: "fl-hint fl-season-now", text: now }) : null);
  }

  function anime() {
    const added = el("div", { class: "chips" }, [...new Set([...POPULAR_TAGS, ...f.tags, ...f.not_tags])].map((t) => triChip(t, "tags", "not_tags")));
    const search = el("input", { class: "fl-search", type: "search", placeholder: "Find another tag", list: "fl-tags", autocomplete: "off" });
    const options = el("datalist", { id: "fl-tags" });
    const fillOptions = () => options.replaceChildren(...(tagList || []).map(([name]) => el("option", { value: name })));
    if (tagList) fillOptions();
    else invoke("media_anime_tags").then((t) => { tagList = t; if (options.isConnected) fillOptions(); }).catch(() => { search.placeholder = "More tags need AniList, which didn't answer"; });
    search.addEventListener("change", () => {
      const hit = (tagList || []).find(([name]) => name.toLowerCase() === search.value.trim().toLowerCase());
      if (hit && !f.tags.includes(hit[0])) change((x) => (x.tags = [...x.tags, hit[0]]));
      search.value = "";
    });
    const parts = [
      field("Format", el("div", { class: "chips" }, FORMATS.map(([v, t]) => el("button", { class: `chip${f.formats.includes(v) ? " on" : ""}`, onclick: () => change((x) => (x.formats = toggle(x.formats, v))) }, t)))),
      field("Airing", oneOf(STATUSES, ([v]) => f.status === v, (x, [v]) => (x.status = v))),
      seasonField(),
      field("Episodes", oneOf(LENGTHS, ([a, b]) => f.episodes_min === a && f.episodes_max === b, (x, [a, b]) => { x.episodes_min = a; x.episodes_max = b; })),
      field("Tags", el("p", { class: "fl-hint", text: "Click once to show only these, twice to leave them out." }), added, el("div", { class: "fl-tag-find" }, search, options)),
    ];
    if (!f.owned) return parts;
    // Your library has no AniList data, so these can't apply to it.
    return [el("p", { class: "fl-hint", text: "Format, airing, season, episodes and tags only work without Only what I have." }), el("div", { class: "fl-off", inert: true }, ...parts)];
  }

  /** Studios (movies) or networks (series): the biggest in the catalog, plus any picked. */
  function studioField(label, hint) {
    const all = [...new Set([...studios, ...f.studios])];
    return field(label, el("p", { class: "fl-hint", text: hint }),
      all.length ? anyChips(all.map((v) => [v, v]), "studios") : el("p", { class: "note", text: "These show up once the catalog has filled in." }));
  }
  const lengthField = (label, options) => field(label,
    oneOf(options, ([a, b]) => f.runtime_min === a && f.runtime_max === b, (x, [a, b]) => { x.runtime_min = a; x.runtime_max = b; }));

  function movies() {
    return [
      field("Out now", oneOf(STAGES, ([v]) => f.status === v, (x, [v]) => (x.status = v))),
      field("Age rating", el("p", { class: "fl-hint", text: "US ratings. G is for everyone, R is for adults." }), anyChips(CERTS.map((c) => [c, c]), "certs")),
      lengthField("Length", MOVIE_LENGTHS),
      studioField("Studio", "The studios with the most movies here."),
    ];
  }

  function series() {
    return [
      field("Airing", oneOf(SHOW_STATUSES, ([v]) => f.status === v, (x, [v]) => (x.status = v))),
      lengthField("Episode length", SHOW_LENGTHS),
      field("Kind of show", el("p", { class: "fl-hint", text: "Click once to show only these, twice to leave them out." }),
        el("div", { class: "chips" }, SHOW_TYPES.map(([v, t]) => triChip(v, "show_types", "not_show_types", t)))),
      studioField("Network or streaming", "Where it first aired."),
      field("Country", el("p", { class: "fl-hint", text: "Where the network or streaming service is based." }), anyChips(COUNTRIES, "countries")),
    ];
  }

  /** Off while Only what I have is on: the library has none of this data. */
  const notOwned = (parts, what) => (f.owned ? [el("p", { class: "fl-hint", text: `${what} only work without Only what I have.` }), el("div", { class: "fl-off", inert: true }, ...parts)] : parts);

  function languageField() {
    const hint = f.kind === "series" ? "The language it was made in. Korean, Japanese and Chinese also find shows from those countries." : "The language it was made in.";
    return field("Language", ...notOwned([el("p", { class: "fl-hint", text: hint }),
      el("div", { class: "chips" }, LANGS.map(([v, t]) => triChip(v, "languages", "not_languages", t)))], "Languages"));
  }

  function drawBar() {
    const owned = el("input", { type: "checkbox", class: "switch", checked: f.owned, "aria-label": "Only what I have" });
    owned.addEventListener("change", () => change((x) => (x.owned = owned.checked), { now: true }));
    const sort = el("select", { class: "filter-select", "aria-label": "Sort by" }, SORTS.map(([v, t]) => el("option", { value: v, text: t, selected: f.sort === v })));
    sort.addEventListener("change", () => change((x) => (x.sort = sort.value), { now: true }));
    const top = el("div", { class: "fl-top" },
      el("div", { class: "segmented", role: "group", "aria-label": "Type" }, KINDS.map(([k, t]) => el("button", { class: f.kind === k ? "on" : "", onclick: () => change((x) => (x.kind = k), { now: true }) }, t))),
      el("label", { class: "fl-inline" }, el("small", { text: "Sort by" }), sort),
      el("label", { class: "fl-inline fl-owned" }, el("small", { text: "Only what I have" }), owned));

    const presets = yearPresets();
    const custom = (f.year_from || f.year_to) && !presets.some(([a, b]) => a === f.year_from && b === f.year_to);
    const advCount = (custom ? 1 : 0) + !!f.status + (f.kind === "anime"
      ? f.tags.length + f.not_tags.length + f.formats.length + !!f.season + !!(f.episodes_min || f.episodes_max)
      : f.certs.length + !!(f.runtime_min || f.runtime_max) + f.studios.length + f.show_types.length + f.not_show_types.length + f.countries.length);
    const own = f.kind === "anime" ? anime() : f.kind === "series"
      ? notOwned(series(), "Airing, length, kind, network and country") : notOwned(movies(), "Out now, age rating, length and studio");
    const adv = el("div", { class: `fl-adv${advancedOpen ? " open" : ""}` }, el("div", { class: "fl-adv-in" },
      field("Years", el("div", { class: "fl-years" }, yearSelect("From", "year_from"), yearSelect("To", "year_to"))), ...own));
    const more = el("button", { class: `btn small soft fl-more${advancedOpen ? " open" : ""}`, "aria-expanded": String(advancedOpen), onclick: () => {
      advancedOpen = !advancedOpen;
      try { localStorage.setItem("chillarr-filter-adv", advancedOpen ? "1" : "0"); } catch (e) { /* storage blocked */ }
      adv.classList.toggle("open", advancedOpen);
      more.classList.toggle("open", advancedOpen);
      more.setAttribute("aria-expanded", String(advancedOpen));
    } }, "More filters", advCount ? el("span", { class: "chip-n", text: String(advCount) }) : null, icon("M6 9l6 6 6-6"));

    const on = pills();
    // replaceChildren writes a null as the word "null", so the empty parts are dropped.
    bar.replaceChildren(...[
      top,
      field("Genres", el("p", { class: "fl-hint", text: "Click once to show only these, twice to leave them out." }),
        el("div", { class: "chips" }, genres.length ? genres.map((g) => triChip(g, "genres", "not_genres")) : el("p", { class: "note", text: "Genres show up once the catalog has filled in." }))),
      f.kind === "anime" ? null : languageField(),
      el("div", { class: "fl-pair" },
        field("Released", oneOf(presets, ([a, b]) => f.year_from === a && f.year_to === b, (x, [a, b]) => { x.year_from = a; x.year_to = b; })),
        field("Rating", oneOf(RATINGS, ([v]) => f.min_rating === v, (x, [v]) => (x.min_rating = v)))),
      more, adv,
      on.length ? el("div", { class: "fl-on" }, ...on, el("button", { class: "btn small ghost", onclick: () => change((x) => Object.assign(x, BLANK), { now: true }) }, "Clear all")) : null].filter(Boolean));
  }

  drawBar();
  if (!mode) { loadResults(); return; }
  const loaded = loadResults(want);
  const seq = currentSeq();
  await loaded;
  // The grid carries on loading by itself once it's on screen.
  if (seq === currentSeq()) swapIn(view, [page], mode);
}
