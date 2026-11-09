// The filter panel both modes share (#40): reading's Manga/Manhwa/Manhua/Novels
// tabs and the Movies, Series and Anime filters. A Filters button with a
// count, folding open a panel under the page header, with an Order dropdown
// beside it; tri-state chips (once: only these, again: leave them out, again:
// clear), one-of chips, More filters and Clear filters.
import { el, getPref, icon, ICON, setPref } from "./api.js";

/** Off -> in -> out -> off, on the filter object `f`. */
export function cycleIn(f, inKey, outKey, v) {
  if (f[inKey].includes(v)) { f[inKey] = f[inKey].filter((g) => g !== v); f[outKey] = [...f[outKey], v]; }
  else if (f[outKey].includes(v)) f[outKey] = f[outKey].filter((g) => g !== v);
  else f[inKey] = [...f[inKey], v];
}

/** Builders bound to one page: `f` is its filter, `change(mut)` applies a change. */
export function chipKit(f, change) {
  const triChip = (v, inKey, outKey, label = v) => {
    const on = f[inKey].includes(v), out = f[outKey].includes(v);
    return el("button", { class: `chip${on ? " on" : ""}${out ? " out" : ""}`, "aria-pressed": on ? "true" : out ? "mixed" : "false",
      title: on ? "Showing only these. Click to leave them out." : out ? "Leaving these out. Click to clear." : "Click to show only these",
      onclick: () => change((x) => cycleIn(x, inKey, outKey, v)) }, ...[out ? icon("M5 12h14") : on ? icon(ICON.check) : null, label].filter(Boolean));
  };
  const oneOf = (options, isOn, pick) => el("div", { class: "chips" }, options.map((o) =>
    el("button", { class: `chip${isOn(o) ? " on" : ""}`, onclick: () => change((x) => pick(x, o)) }, o[o.length - 1])));
  // Any of these: click to add, again to take off.
  const anyOf = (options, key) => el("div", { class: "chips" }, options.map(([v, t]) =>
    el("button", { class: `chip${f[key].includes(v) ? " on" : ""}`, "aria-pressed": String(f[key].includes(v)),
      onclick: () => change((x) => (x[key] = x[key].includes(v) ? x[key].filter((y) => y !== v) : [...x[key], v])) }, t)));
  const field = (label, ...kids) => el("div", { class: "fl-field" }, el("h3", { text: label }), ...kids.filter(Boolean));
  const row = (...kids) => el("div", { class: "fl-row" }, ...kids.filter(Boolean));
  const yearBox = (key, label) => {
    const i = el("input", { class: "read-from", type: "number", min: "1900", max: "2100", step: "1", placeholder: label, "aria-label": label, value: f[key] ? String(f[key]) : "" });
    i.addEventListener("change", () => change((x) => { const n = parseInt(i.value, 10); x[key] = n >= 1900 && n <= 2100 ? n : null; }));
    return i;
  };
  const years = (fromKey = "year_from", toKey = "year_to") => el("div", { class: "read-years" }, yearBox(fromKey, "From"), el("span", { text: "to" }), yearBox(toKey, "To"));
  return { triChip, oneOf, anyOf, field, row, years };
}

/** The Filters button: shows the count, folds `panel` open or shut, and
 *  remembers which per profile under `pref`. */
export function filtersButton(panel, pref) {
  let open = getPref(pref) === "1";
  const badge = el("span", { class: "read-fl-count" });
  const button = el("button", { class: "btn soft read-fl-toggle", "aria-expanded": String(open), onclick: () => {
    open = !open;
    setPref(pref, open ? "1" : "0");
    button.setAttribute("aria-expanded", String(open));
    panel.classList.toggle("shut", !open);
  } }, icon("M4 6h16M7 12h10M10 18h4"), "Filters", badge);
  panel.classList.toggle("shut", !open);
  const count = (n) => { badge.textContent = n ? String(n) : ""; badge.hidden = !n; };
  return { button, count };
}

/** The Order dropdown. */
export function orderSelect(options, now, pick) {
  const s = el("select", { class: "quality", "aria-label": "Order" }, options.map(([v, t]) => el("option", { value: v, text: t, selected: now === v })));
  s.addEventListener("change", () => pick(s.value));
  return s;
}

/** "More filters", folding the rest open under it. */
export const moreButton = (open, flip) =>
  el("button", { class: "linkish fl-more" + (open ? " open" : ""), "aria-expanded": String(open), onclick: flip }, "More filters", icon("m6 9 6 6 6-6"));

export const clearButton = (onclick) => el("button", { class: "linkish read-fl-clear", onclick }, "Clear filters");

/** The 18+ choice, the same everywhere: off until picked. */
export const ADULT_CHOICES = [["hide", "Not included"], ["include", "Included"], ["only", "Only 18+"]];
/** Porn inside 18+: off until picked, and only there while 18+ is on. */
export const PORN_CHOICES = [["hide", "Not included"], ["include", "Included"]];

/** Porn is on: 18+ is on and Porn is Included. */
export const pornOn = (f) => !!f.adult && f.adult !== "hide" && f.porn === "include";

/** The 18+ field, and Porn under it once 18+ is on. `porn` false leaves Porn
    out where the catalog can't tell porn apart. `kit` is the page's chipKit. */
export function adultFields(f, kit, porn, hint) {
  const out = [kit.field("18+", kit.oneOf(ADULT_CHOICES, (o) => f.adult === o[0], (x, o) => (x.adult = o[0])),
    el("small", { class: "read-fl-hint", text: hint || "Off until you pick Included or Only 18+." }))];
  if (porn && f.adult !== "hide") {
    out.push(kit.field("Porn", kit.oneOf(PORN_CHOICES, (o) => (f.porn || "hide") === o[0], (x, o) => (x.porn = o[0])),
      el("small", { class: "read-fl-hint", text: "Off until you pick Included." })));
  }
  return out;
}
