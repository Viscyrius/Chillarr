// Picking several at once: a Select button on a row or grid puts a tick on
// each of its cards, a click (or Shift and a click for a stretch) picks them,
// and a bar along the bottom says how many and what can be done with them all
// (delete their downloads, stop following, delete their files). One selection
// at a time; Esc, Done or leaving the page ends it.
import { el, icon, ICON } from "./api.js";

let active = null;
window.addEventListener("hashchange", () => active?.stop());
// Esc ends it, unless a question about it is up (that Esc belongs to the question).
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || !active || e.defaultPrevented || active.busy() || document.querySelector("dialog[open]")) return;
  active.stop();
});

/** Ends the selection on screen, if any (its cards are being redrawn). */
export function stopSelecting() { active?.stop(); }

/**
 * The Select button for `host`. `cardsOf()` gives [node, item] for every card
 * that can be picked; each action is { label, icon, danger, run(items) },
 * `run` resolving true when it's done (the selection then ends).
 */
export function selectable(host, cardsOf, actions, label = "Select") {
  const btn = el("button", { class: "row-select", "aria-pressed": "false", title: "Pick several to act on them all" }, icon("M4 12l5 5L20 6"), label);
  btn.addEventListener("click", (e) => { e.preventDefault(); if (active && active.host === host) active.stop(); else start(host, btn, cardsOf, actions); });
  return btn;
}

function start(host, btn, cardsOf, actions) {
  active?.stop();
  // Only cards you can see: one hidden from this profile is never picked by Pick all or a Shift range.
  const cards = cardsOf().filter(([n, item]) => n && item && n.getClientRects().length);
  if (!cards.length) return;
  const picked = new Set();
  let last = -1;
  const count = el("strong", { class: "sel-count" });
  const all = el("button", { class: "linkish" });
  const doing = actions.map((a) => {
    const b = el("button", { class: a.danger ? "btn small danger-fill" : "btn small soft" }, a.icon ? icon(a.icon) : null, a.label);
    b.addEventListener("click", async () => {
      if (!picked.size) return;
      const items = cards.filter(([n]) => picked.has(n)).map(([, item]) => item);
      bar.classList.add("busy");
      buttons.forEach((x) => (x.disabled = true));
      let done = false;
      try { done = await a.run(items); } finally {
        if (done) stop();
        else { bar.classList.remove("busy"); paint(); }
      }
    });
    return b;
  });
  const done = el("button", { class: "btn small ghost", onclick: () => stop() }, "Done");
  const buttons = [...doing, all, done];
  const bar = el("div", { class: "sel-bar", role: "toolbar", "aria-label": "Picked" }, count, all, el("span", { class: "sel-gap" }), ...doing, done);
  const ticks = [];
  for (const [n] of cards) {
    n.classList.add("sel-able");
    const t = el("span", { class: "sel-tick", "aria-hidden": "true" }, icon(ICON.check));
    (n.querySelector(".art") || n).append(t);
    ticks.push(t);
  }
  const index = (n) => cards.findIndex(([x]) => x === n);
  function toggle(n, shift) {
    const i = index(n);
    if (i < 0) return;
    const on = !picked.has(n);
    // Shift and a click: everything between the last one picked and this one.
    if (shift && last >= 0) {
      const [a, b] = [Math.min(last, i), Math.max(last, i)];
      for (let j = a; j <= b; j++) { if (on) picked.add(cards[j][0]); else picked.delete(cards[j][0]); }
    } else if (on) picked.add(n); else picked.delete(n);
    last = i;
    paint();
  }
  function paint() {
    for (const [n] of cards) { n.classList.toggle("sel-on", picked.has(n)); n.setAttribute("aria-pressed", String(picked.has(n))); }
    count.textContent = picked.size ? `${picked.size} picked` : "Pick some";
    all.textContent = picked.size === cards.length ? "Pick none" : "Pick all";
    doing.forEach((b) => (b.disabled = !picked.size));
    all.disabled = false;
    done.disabled = false;
  }
  all.addEventListener("click", () => {
    if (picked.size === cards.length) picked.clear(); else for (const [n] of cards) picked.add(n);
    paint();
  });
  // In front of the cards' own listeners: a click picks, a hover opens no card.
  const onClick = (e) => {
    const n = e.target.closest(".sel-able");
    if (!n || !host.contains(n) || e.target.closest(".week-hide")) return;
    e.preventDefault();
    e.stopPropagation();
    toggle(n, e.shiftKey);
  };
  const quiet = (e) => { if (e.target.closest?.(".sel-able")) e.stopPropagation(); };
  host.addEventListener("click", onClick, true);
  host.addEventListener("mouseenter", quiet, true);
  host.classList.add("selecting");
  btn.setAttribute("aria-pressed", "true");
  btn.classList.add("on");
  document.body.append(bar);
  requestAnimationFrame(() => bar.classList.add("in"));
  paint();
  function stop() {
    if (active !== self) return;
    active = null;
    host.removeEventListener("click", onClick, true);
    host.removeEventListener("mouseenter", quiet, true);
    host.classList.remove("selecting");
    btn.setAttribute("aria-pressed", "false");
    btn.classList.remove("on");
    for (const [n] of cards) { n.classList.remove("sel-able", "sel-on"); n.removeAttribute("aria-pressed"); }
    ticks.forEach((t) => t.remove());
    bar.classList.remove("in");
    setTimeout(() => bar.remove(), 220);
  }
  const self = { host, stop, busy: () => bar.classList.contains("busy") };
  active = self;
}
