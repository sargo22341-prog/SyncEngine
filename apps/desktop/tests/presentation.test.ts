import { test } from "node:test";
import assert from "node:assert/strict";
import { escape, errorMessage, runStatus } from "../src/presentation.ts";
test("untrusted file names, profiles and errors cannot inject HTML", () => {
  assert.equal(escape('<img src=x onerror="alert(1)">&\''), "&lt;img src=x onerror=&quot;alert(1)&quot;&gt;&amp;&#39;");
});
test("backend failures retain category and profile context", () => {
  assert.equal(errorMessage({ code: "busy", message: "Déjà en cours", profile: "p1" }), "busy : Déjà en cours (profil p1)");
});
test("partial or uncertain work is never displayed as success", () => {
  assert.match(runStatus({ state: "running" }), /diagnostic/);
  assert.match(runStatus({ state: "finished", report: { status: "cancelled" } }), /partiel/);
  assert.match(runStatus({ state: "finished", report: { execution: { status: "succeeded" }, verification_error: "rescan failed" } }), /Échec/);
  assert.equal(runStatus({ state: "finished", report: { status: "succeeded" } }), "Réussi");
});
