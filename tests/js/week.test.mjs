// Run: node --test tests/js
import test from "node:test";
import assert from "node:assert/strict";
import { usualDay } from "../../src/js/week.js";

const at = (iso) => ({ uploaded: String(new Date(iso).getTime()) });

test("the usual day, not the newest chapter's", () => {
  // Saturdays, then one late on a Friday: still Saturday.
  const ch = [at("2026-11-20T18:44"), at("2026-11-14T14:45"), at("2026-11-07T19:26"), at("2026-10-31T10:00"), at("2026-10-24T09:00")];
  assert.equal(usualDay(ch), 6);
});

test("chapters a site stamped together don't count", () => {
  const batch = Array.from({ length: 6 }, () => at("2026-11-15T15:08"));
  assert.equal(usualDay(batch), -1, "only a batch: no day");
  const mixed = [...batch, at("2026-11-11T20:00"), at("2026-11-04T20:00"), at("2026-10-28T20:00")];
  assert.equal(usualDay(mixed), 3, "Wednesdays, the Sunday batch left out");
});

test("too little to tell", () => {
  assert.equal(usualDay([]), -1);
  assert.equal(usualDay([at("2026-11-11T20:00")]), -1);
  assert.equal(usualDay([{ uploaded: null }, { uploaded: "0" }]), -1);
});
