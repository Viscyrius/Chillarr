// A title page's buttons, tidied: the few you use every time stay in the row
// (Play/Resume, Follow/Download, My List, Recommend, Trailer), the rest go
// into one ⋯ button. On a PC it opens as a menu under the button; on a phone
// it slides up from the bottom with rows big enough to tap.
//
// Nothing about a button changes: each one is still made, drawn and kept up
// to date by the code that made it. It is only hidden from the row, and the
// menu's row for it clicks it. So every button keeps working exactly as
// before, including ones added later (Trailer, Delete) and ones that change
// their own label (Get new chapters).
import { el } from "./api.js";

const KEEP_TEXT = /^(Start over|Search again|Watch now)$/;
const DANGER_TEXT = /^(Stop following|Delete|Remove)\b/;

/** Phones and small windows get the bottom sheet. */
export const sheetMode = () => matchMedia("(max-width: 600px), (hover: none) and (max-width: 980px)").matches;

/** Stays in the row: the main action, My List, Recommend, Trailer, controls that aren't buttons. */
function stays(b) {
  if (b.classList.contains("more-btn")) return true;
  if (b.tagName !== "BUTTON" || !b.classList.contains("btn")) return true;
  const c = b.classList;
  if (c.contains("play") || c.contains("primary") || c.contains("save-btn") || c.contains("list-btn") || c.contains("rec-btn") || c.contains("trailer-btn")) return true;
  // A disabled button in the row is a status ("Downloading", "Missing").
  if (b.disabled && !b.textContent.trim().match(/^(Get|Delete|Hide|Show|Mark)/)) return true;
  return KEEP_TEXT.test(b.textContent.trim());
}

const isDanger = (b) => b.classList.contains("danger") || b.classList.contains("danger-fill") || DANGER_TEXT.test(b.textContent.trim());

/** Tidies `actions` now and whenever buttons are added to it. */
export function tidy(actions) {
  if (actions.dataset.tidy) return;
  actions.dataset.tidy = "1";
  actions.classList.add("tidy");
  const more = el("button", { class: "btn soft more-btn", type: "button", "aria-label": "More options", "aria-haspopup": "menu", title: "More" },
    dots(), el("span", { class: "more-label", text: "More" }));
  more.addEventListener("click", () => openMenu(more, actions));
  const sort = () => {
    for (const b of actions.children) {
      if (b === more) continue;
      if (!stays(b)) b.classList.add("in-more");
    }
    // ⋯ goes last, and only when it has something in it.
    if (more.parentNode !== actions || actions.lastElementChild !== more) actions.append(more);
    more.hidden = !actions.querySelector(":scope > .in-more");
  };
  new MutationObserver((list) => {
    if (list.every((m) => [...m.addedNodes, ...m.removedNodes].every((n) => n === more))) return;
    sort();
  }).observe(actions, { childList: true });
  sort();
}

function dots() {
  const s = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  s.setAttribute("viewBox", "0 0 24 24");
  s.setAttribute("aria-hidden", "true");
  s.setAttribute("class", "dots");
  for (const x of [5, 12, 19]) {
    const c = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    c.setAttribute("cx", x); c.setAttribute("cy", "12"); c.setAttribute("r", "2");
    s.append(c);
  }
  return s;
}

/** The ⋯ menu (or sheet) for the hidden buttons, built fresh so each label is current. */
function openMenu(anchor, actions) {
  const hidden = [...actions.querySelectorAll(":scope > .in-more")];
  const row = (b) => {
    const pressed = b.getAttribute("aria-pressed");
    const svg = b.querySelector("svg");
    const item = el("button", { class: "more-item" + (isDanger(b) ? " danger" : ""), type: "button", role: "menuitem", disabled: b.disabled, title: b.title || null },
      svg ? svg.cloneNode(true) : el("span", { class: "more-noicon" }),
      el("span", { class: "more-text", text: b.textContent.trim() }),
      pressed != null || b.classList.contains("read-live") ? el("span", { class: "more-switch" + (pressed === "true" || b.classList.contains("on") ? " on" : "") }) : null);
    item.addEventListener("click", () => { close(); b.click(); });
    return item;
  };
  const safe = hidden.filter((b) => !isDanger(b)).map(row);
  const risky = hidden.filter(isDanger).map(row);
  const sheet = sheetMode();
  const d = el("dialog", { class: "more-menu" + (sheet ? " sheet" : ""), role: "menu", "aria-label": "More options" },
    sheet ? el("span", { class: "more-grab", "aria-hidden": "true" }) : null,
    ...safe, safe.length && risky.length ? el("span", { class: "more-sep", role: "separator" }) : null, ...risky);
  const close = () => { d.classList.add("closing"); setTimeout(() => d.remove(), 160); if (d.open) d.close(); anchor.setAttribute("aria-expanded", "false"); };
  d.addEventListener("click", (e) => { if (e.target === d) close(); });
  d.addEventListener("cancel", (e) => { e.preventDefault(); close(); });
  document.body.append(d);
  d.showModal();
  anchor.setAttribute("aria-expanded", "true");
  if (!sheet) {
    const r = anchor.getBoundingClientRect();
    const w = d.offsetWidth;
    const left = Math.max(12, Math.min(r.right - w, innerWidth - w - 12));
    const below = r.bottom + 8;
    const top = below + d.offsetHeight > innerHeight - 12 ? Math.max(12, r.top - d.offsetHeight - 8) : below;
    d.style.left = `${left}px`;
    d.style.top = `${top}px`;
  }
  (d.querySelector(".more-item:not(:disabled)") || d).focus();
  d.addEventListener("keydown", (e) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const items = [...d.querySelectorAll(".more-item:not(:disabled)")];
    const i = items.indexOf(document.activeElement);
    items[(i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
    e.preventDefault();
  });
}
