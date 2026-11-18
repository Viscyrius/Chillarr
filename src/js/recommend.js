// Recommend: send a title to other profiles on this Chillarr, with a note,
// and see what they sent you ("Recommended for you") and what they've been
// watching and reading ("Your profiles are into"). recommend.rs keeps it.
import { el, icon, invoke, toast } from "./api.js";
import { isHidden } from "./hidden.js";
import { sheetMode } from "./more.js";

const SEND = "M4 12l16-8-6 16-3-7z";
const SHUT = "M6 6l12 12M18 6L6 18";

/** A card as it's sent: the fields a page needs to open it again, nothing heavy. */
function slim(kind, c) {
  const keep = kind === "read"
    ? ["key", "anilist", "title", "names", "cover", "banner", "country", "format", "status", "description", "genres", "adult"]
    : ["key", "kind", "title", "subtitle", "year", "overview", "poster", "backdrop", "rating", "jf_id", "series_id", "tmdb", "tvdb", "format", "genres", "trailer", "adult"];
  const out = {};
  for (const k of keep) if (c[k] != null) out[k] = k === "description" || k === "overview" ? String(c[k]).slice(0, 1500) : c[k];
  // An episode is recommended as its show.
  if (kind === "media" && c.kind === "episode" && c.series_id) { out.kind = "series"; out.jf_id = c.series_id; delete out.subtitle; }
  return out;
}

/** The Recommend button for a title page. */
export function recommendButton(kind, c) {
  if (!c || !c.title) return null;
  const b = el("button", { class: "btn soft rec-btn", type: "button", title: "Send it to another profile" }, icon(SEND), "Recommend");
  b.addEventListener("click", () => openRecommend(kind, c));
  return b;
}

/** The picker: who to send it to, and a note. */
export async function openRecommend(kind, c) {
  let people;
  try { people = await invoke("recommend_targets", { adult: !!(c.adult || c.isAdult) }); } catch (e) { toast(String(e), true); return; }
  if (!people.length) { toast("There are no other profiles to send it to yet. Add one from the profile picker."); return; }
  const picked = new Set();
  const send = el("button", { class: "btn primary", type: "button", disabled: true }, icon(SEND), "Send");
  const paint = () => {
    send.disabled = !picked.size;
    send.lastChild.textContent = picked.size ? `Send to ${picked.size} profile${picked.size === 1 ? "" : "s"}` : "Send";
  };
  const tiles = people.map((p) => {
    const t = el("button", { class: "rec-person", type: "button", "aria-pressed": "false" },
      el("span", { class: "rec-avatar", style: `background:${p.color}`, text: (p.name || "?").slice(0, 1).toUpperCase() }),
      el("span", { class: "rec-name", text: p.name }),
      el("span", { class: "rec-tick", "aria-hidden": "true" }, icon("M5 12l5 5L20 7")));
    t.addEventListener("click", () => {
      if (picked.has(p.id)) picked.delete(p.id); else picked.add(p.id);
      t.setAttribute("aria-pressed", String(picked.has(p.id)));
      paint();
    });
    return t;
  });
  const note = el("textarea", { class: "rec-note", rows: "3", maxlength: "280", placeholder: "Why they'd like it (optional)" });
  const art = c.cover || c.poster;
  const d = el("dialog", { class: "rec-dlg" + (sheetMode() ? " sheet" : ""), "aria-label": `Recommend ${c.title}` },
    el("div", { class: "rec-head" },
      art ? el("img", { class: "rec-art", src: art, alt: "" }) : null,
      el("div", { class: "rec-titles" }, el("h2", { text: `Recommend ${c.title}` }), el("p", { text: "It shows on their home page with your name on it." })),
      el("button", { class: "rec-x", type: "button", "aria-label": "Close", onclick: () => shut() }, icon(SHUT))),
    el("div", { class: "rec-people" }, tiles),
    el("label", { class: "rec-note-wrap" }, el("span", { text: "Add a note" }), note),
    el("div", { class: "rec-go" }, el("button", { class: "btn ghost", type: "button", onclick: () => shut() }, "Cancel"), send));
  const shut = () => { if (d.open) d.close(); setTimeout(() => d.remove(), 200); };
  d.addEventListener("click", (e) => { if (e.target === d) shut(); });
  d.addEventListener("close", () => setTimeout(() => d.remove(), 200));
  send.addEventListener("click", async () => {
    send.disabled = true;
    try {
      toast(await invoke("recommend_send", { kind, card: slim(kind, c), to: [...picked], note: note.value || null }));
      shut();
    } catch (e) { toast(String(e), true); paint(); }
  });
  document.body.append(d);
  d.showModal();
  tiles[0]?.focus();
}

/** "[Name] and [Name] are reading this", filled in when it's known. */
export function whoLine(kind, c) {
  const line = el("p", { class: "who-line", hidden: true });
  if (!c) return line;
  invoke("recommend_who", { kind, card: slim(kind, c) }).then((who) => {
    if (!who.length) return;
    const names = who.map((w) => w.name);
    const said = names.length === 1 ? names[0] : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
    const verb = kind === "read" ? (who.length === 1 ? "is reading this" : "are reading this")
      : who.length === 1 ? (who[0].what === "watched" ? "watched this" : who[0].what.startsWith("on ") ? `is ${who[0].what}` : "is watching this")
        : "have watched this";
    line.replaceChildren(el("span", { class: "who-dots" }, who.slice(0, 4).map((w) => el("i", { style: `background:${w.color}`, title: w.name }))), el("span", { text: `${said} ${verb}` }));
    line.hidden = false;
  }).catch(() => {});
  return line;
}

const nameOf = (r) => r.from_name || "Someone";

/**
 * The home page's two rows from other profiles. `kind` is "media" (Home) or
 * "read" (Reading); `open` opens a card the way that page does. The section
 * removes itself when there's nothing to show.
 */
export function profileRows(kind, open, alive = () => true) {
  const box = el("div", { class: "pf-rows" });
  Promise.all([invoke("recommend_inbox").catch(() => []), invoke("recommend_into").catch(() => [])]).then(([inbox, into]) => {
    if (!alive()) return;
    const recs = inbox.filter((r) => r.kind === kind && !isHidden(r.card));
    const near = into.filter((x) => x.kind === kind && !isHidden(x.card));
    const parts = [];
    if (recs.length) {
      const track = el("div", { class: "rec-track" });
      const draw = () => {
        track.replaceChildren(...recs.map((r) => recCard(r, open, () => { recs.splice(recs.indexOf(r), 1); if (recs.length) draw(); else sec.remove(); })));
      };
      const sec = el("section", { class: "row pf-recs" }, el("div", { class: "row-head" }, el("h2", { text: "Recommended for you" }), el("span", { class: "count", text: "From the other profiles here" })), track);
      draw();
      parts.push(sec);
    }
    if (near.length) {
      parts.push(el("section", { class: "row pf-into" },
        el("div", { class: "row-head" }, el("h2", { text: kind === "read" ? "Your profiles are reading" : "Your profiles are into" }), el("span", { class: "count", text: "What the others here watched and read lately" })),
        el("div", { class: "into-track" }, near.map((x) => intoCard(x, open)))));
    }
    if (!parts.length) { box.remove(); return; }
    box.replaceChildren(...parts);
  });
  return box;
}

function recCard(r, open, gone) {
  const c = r.card || {};
  const art = c.cover || c.poster;
  const go = () => open(c);
  const dismiss = async () => {
    try { await invoke("recommend_dismiss", { id: r.id }); gone(); } catch (e) { toast(String(e), true); }
  };
  return el("article", { class: "rec-card" },
    el("button", { class: "rec-card-art", type: "button", "aria-label": `Open ${c.title}`, onclick: go }, art ? el("img", { src: art, alt: "", loading: "lazy" }) : el("span", { text: c.title })),
    el("div", { class: "rec-card-body" },
      el("strong", { text: c.title }),
      el("span", { class: "rec-from" }, el("i", { style: `background:${r.from_color}` }), `${nameOf(r)} recommends it`),
      r.note ? el("q", { class: "rec-quote", text: r.note }) : null,
      el("div", { class: "rec-card-go" },
        el("button", { class: "btn small primary", type: "button", onclick: go }, "Open"),
        el("button", { class: "btn small ghost", type: "button", onclick: dismiss }, "Dismiss"))));
}

function intoCard(x, open) {
  const c = x.card || {};
  const art = c.cover || c.poster;
  const who = x.who.map((w) => `${w.name}${w.what && w.what !== "reading" && w.what !== "watching" ? ` (${w.what})` : ""}`).join(", ");
  return el("button", { class: "into-card", type: "button", title: who, onclick: () => open(c) },
    el("span", { class: "into-art" }, art ? el("img", { src: art, alt: "", loading: "lazy" }) : null,
      el("span", { class: "who-dots" }, x.who.slice(0, 4).map((w) => el("i", { style: `background:${w.color}` })))),
    el("span", { class: "into-title", text: c.title }),
    el("span", { class: "into-who", text: who }));
}
