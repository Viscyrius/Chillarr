// Read Party (readparty.rs): reading a series together, like Watch together
// is for films. Start one from the reader (the people button) and share the
// code; friends join from Read together in the top bar. Everyone is on the
// same chapter and page: whoever turns the page moves the party.
import { el, icon, invoke, toast } from "./api.js";
import { openReader } from "./reader.js";

const PEOPLE = "M16 19v-1a4 4 0 0 0-4-4H7a4 4 0 0 0-4 4v1M12.5 7a3 3 0 1 1-6 0 3 3 0 0 1 6 0M21 19v-1a4 4 0 0 0-3-3.9M15.5 4.1a3 3 0 0 1 0 5.8";
let party = null; // { code, key, title, ver, people }
let hooks = null; // the open reader's: where() and go(chapter, page)
let pollT = 0;
let sendT = 0;
let quietUntil = 0;

export const partyIcon = () => icon(PEOPLE);
export const inParty = () => !!party;

/** The reader, while it's open: how to read where it is, and how to move it. */
export function attachReader(h) { hooks = h; if (party) poll(); }
export function detachReader() { hooks = null; }

/** The reader turned a page: the party moves with it. */
export function moved() {
  if (!party || !hooks || Date.now() < quietUntil) return;
  clearTimeout(sendT);
  sendT = setTimeout(async () => {
    const w = hooks && hooks.where();
    if (!w || !party || w.key !== party.key) return;
    try { take(await invoke("readparty_move", { code: party.code, chapter: w.chapter, page: w.page })); } catch (e) { ended(e); }
  }, 350);
}

function take(s) {
  if (!party) return;
  party.ver = Math.max(party.ver, s.ver);
  party.people = s.people;
  paint();
}

async function poll() {
  clearTimeout(pollT);
  if (!party) return;
  try {
    const s = await invoke("readparty_state", { code: party.code });
    if (!party) return;
    const newer = s.ver > party.ver;
    party.people = s.people;
    party.ver = Math.max(party.ver, s.ver);
    // Someone else moved: follow them (a page landing slightly off doesn't bounce back).
    const w = hooks && hooks.where();
    if (newer && !s.mine && w && w.key === s.key && (w.chapter !== s.chapter || w.page !== s.page)) {
      quietUntil = Date.now() + 1500;
      hooks.go(s.chapter, s.page);
    }
    paint();
  } catch (e) { ended(e); return; }
  pollT = setTimeout(poll, document.hidden ? 4000 : 1000);
}

function ended(e) {
  if (!party) return;
  party = null;
  clearTimeout(pollT);
  paint();
  toast(String(e).includes("ended") ? "The reading party ended." : String(e), true);
}

async function start() {
  const w = hooks && hooks.where();
  if (!w) { toast("Open a chapter first, then start a reading party from the reader."); return; }
  try {
    const s = await invoke("readparty_start", { key: w.key, chapter: w.chapter, page: w.page });
    party = { code: s.code, key: s.key, title: s.title, ver: s.ver, people: s.people };
    paint();
    poll();
    panel();
  } catch (e) { toast(String(e), true); }
}

async function join(code) {
  try {
    const s = await invoke("readparty_join", { code });
    party = { code: s.code, key: s.key, title: s.title, ver: s.ver, people: s.people };
    close();
    // Open where the party is, then keep in step.
    await openReader(s.key, s.chapter);
    quietUntil = Date.now() + 1500;
    if (hooks) hooks.go(s.chapter, s.page);
    paint();
    poll();
    toast(`You're reading ${s.title} together.`);
  } catch (e) { toast(String(e), true); }
}

async function leave() {
  if (!party) return;
  const code = party.code;
  party = null;
  clearTimeout(pollT);
  paint();
  close();
  invoke("readparty_leave", { code }).catch(() => {});
  toast("You left the reading party.");
}

/* ---------- the panel ---------- */
let dlg = null;
function close() { if (dlg && dlg.open) dlg.close(); }

/** Read together: the code to share and Leave, or a box to join a friend's. */
export function panel() {
  if (!dlg) {
    dlg = el("dialog", { class: "ask rp-dialog", "aria-label": "Read together" });
    dlg.addEventListener("click", (e) => { if (e.target === dlg) close(); });
    document.body.append(dlg);
  }
  const head = el("div", { class: "rp-head" }, el("span", { class: "rp-icon" }, icon(PEOPLE)), el("h2", { text: "Read together" }));
  if (party) {
    const copy = el("button", { class: "btn soft", onclick: async () => {
      try { await navigator.clipboard.writeText(party.code); copy.textContent = "Copied"; } catch (e) { toast(`The code is ${party.code}`); }
    } }, "Copy code");
    dlg.replaceChildren(head,
      el("p", { class: "sub", text: `Reading ${party.title}. Share this code: anyone who joins follows the same page, and so do you.` }),
      el("div", { class: "rp-code", text: party.code }),
      el("p", { class: "rp-people", text: party.people === 1 ? "Just you so far" : `${party.people} reading together` }),
      el("div", { class: "rp-acts" }, copy, el("button", { class: "btn ghost", onclick: leave }, "Leave"), el("button", { class: "btn primary", onclick: close }, "Done")));
  } else {
    const input = el("input", { class: "rp-input", type: "text", maxlength: "8", placeholder: "Code", "aria-label": "Party code", autocomplete: "off", spellcheck: "false" });
    const go = el("button", { class: "btn primary", onclick: () => input.value.trim() && join(input.value) }, "Join");
    input.addEventListener("keydown", (e) => { if (e.key === "Enter" && input.value.trim()) { e.preventDefault(); join(input.value); } });
    const startBtn = hooks ? el("button", { class: "btn soft", onclick: start }, "Start one here") : null;
    dlg.replaceChildren(head,
      el("p", { class: "sub", text: hooks ? "Start a party on this chapter and share the code, or join a friend's." : "Got a code from a friend? Join their party. To start one, open a chapter and press the people button in the reader." }),
      el("div", { class: "rp-join" }, input, go),
      el("div", { class: "rp-acts" }, startBtn, el("button", { class: "btn ghost", onclick: close }, "Close")));
    requestAnimationFrame(() => input.focus());
  }
  if (!dlg.open) dlg.showModal();
}

/** The buttons show when you're in a party, and how many are reading. */
function paint() {
  for (const b of document.querySelectorAll(".rp-btn")) {
    b.classList.toggle("on", !!party);
    b.title = party ? `Reading together (${party.people})` : "Read together";
    let n = b.querySelector(".rp-n");
    if (party && party.people > 1) {
      if (!n) { n = el("span", { class: "rp-n" }); b.append(n); }
      n.textContent = String(party.people);
    } else if (n) n.remove();
  }
}

// The top bar's Read together, next to Watch together (shown while reading).
(function topButton() {
  const watch = document.getElementById("together-btn");
  if (!watch) return;
  const b = el("button", { class: "icon-btn rp-btn rp-top", id: "read-together", "aria-label": "Read together", title: "Read together", onclick: () => panel() }, icon(PEOPLE));
  watch.after(b);
})();
