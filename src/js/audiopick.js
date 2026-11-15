// Which audio track to start in. Pure, so it can be tested on its own
// (tests/js/audiopick.test.mjs); player.js asks it on every new title.

/** Languages offered for dubs, with how a file's audio track names them. */
export const AUDIO_LANGS = [
  ["eng", "English", code("en|eng")], ["spa", "Spanish", code("es|spa|esp")], ["fre", "French", code("fr|fre|fra")], ["ger", "German", code("de|ger|deu")],
  ["ita", "Italian", code("it|ita")], ["por", "Portuguese", code("pt|por")], ["rus", "Russian", code("ru|rus")], ["hin", "Hindi", code("hi|hin")],
  ["ara", "Arabic", code("ar|ara")], ["tur", "Turkish", code("tr|tur")], ["jpn", "Japanese", code("ja|jp|jpn")], ["kor", "Korean", code("ko|kor")], ["zho", "Chinese", code("zh|zho|chi|chs|cht")],
];

/**
 * A whole language code ("ru", "rus", "ru-RU"), never the start of another
 * one: "rum" is Romanian, "est" Estonian, "arm" Armenian.
 */
function code(codes) {
  return new RegExp(`^(${codes})(?![a-z])`, "i");
}

/** Tags that say nothing about the language: the track's name is asked instead. */
const NO_TAG = /^(und|mis|mul|zxx)$/i;
const tagOf = (track) => (NO_TAG.test(track.lang || "") ? "" : track.lang || "");

export const isEnglish = (lang) => !lang || AUDIO_LANGS[0][2].test(lang);

/** The dub language's name for a message: "English". */
export const dubName = (lang) => AUDIO_LANGS.find(([c]) => c === (lang || "eng"))?.[1] || "dubbed";

/** True when a track is in `code`: by its language tag, or by its name when it has no tag. */
export function speaks(track, lang) {
  const row = AUDIO_LANGS.find(([c]) => c === lang);
  const tag = tagOf(track);
  if (!row) return tag === lang;
  if (tag) return row[2].test(tag);
  return new RegExp(`(^|[^a-z])${row[1]}([^a-z]|$)`, "i").test(track.label || "");
}

/**
 * `tracks`: the file's audio ({index, lang, label, default}). `settings`:
 * {audio_pref, audio_lang}. `handLang`: the language picked by hand for this
 * show, or null. Returns the track to switch to (null: keep the file's own
 * start) and whether the dub asked for isn't in the file at all.
 */
export function chooseAudio(tracks, settings, handLang) {
  const def = tracks.find((a) => a.default) || tracks[0];
  if (!def) return { index: null, missing: false };
  // A language picked by hand wins, also when the file already starts in it.
  if (handLang) {
    if (speaks(def, handLang)) return { index: null, missing: false };
    const t = tracks.find((a) => speaks(a, handLang));
    if (t) return { index: t.index, missing: false };
  }
  if (settings?.audio_pref === "dub") {
    const want = settings.audio_lang || "eng";
    if (speaks(def, want)) return { index: null, missing: false };
    const t = tracks.find((a) => speaks(a, want));
    return t ? { index: t.index, missing: false } : { index: null, missing: true };
  }
  // Original voices: only languages films are rarely dubbed into, so an English
  // film with an extra Hindi or French track stays in English.
  if (!isEnglish(def.lang)) return { index: null, missing: false };
  const t = tracks.find((a) => /^(ja|jp|jpn|ko|kor|zh|zho|chi|chs|cht)(?![a-z])/i.test(a.lang || ""));
  return { index: t ? t.index : null, missing: false };
}
