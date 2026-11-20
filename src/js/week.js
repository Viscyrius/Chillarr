// Your week: which day a series' chapters usually come out. Kept apart so a
// test can load it without the page.

/** The weekday a series' chapters usually come out (0 = Sunday), or -1 when
 *  its dates can't tell. The most common day of its last six releases, not
 *  the newest one alone: one chapter a day late used to move the series to
 *  another day. Chapters a site stamped together (three or more in the same
 *  hour) are when the site copied them, not when they came out, so they
 *  don't count. */
export function usualDay(chapters) {
  const ts = chapters.map((c) => Number(c.uploaded) || 0).filter((t) => t > 0);
  const perHour = new Map();
  for (const t of ts) { const h = Math.floor(t / 3.6e6); perHour.set(h, (perHour.get(h) || 0) + 1); }
  const days = [...new Set(ts.filter((t) => perHour.get(Math.floor(t / 3.6e6)) < 3).map((t) => new Date(t).toDateString()))]
    .map((s) => new Date(s)).sort((a, b) => b - a).slice(0, 6);
  if (days.length < 2) return -1;
  // Ties go to the more recent day.
  const score = [0, 0, 0, 0, 0, 0, 0];
  days.forEach((d, i) => { score[d.getDay()] += 1 + (days.length - i) / 100; });
  return score.indexOf(Math.max(...score));
}
