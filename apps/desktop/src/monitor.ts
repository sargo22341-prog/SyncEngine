import type { Snapshot, Profile, Preview } from "./api";
import { escape as e } from "./presentation";
let latest: Snapshot = { active: false, phase: "idle", entries_done: 0, bytes_done: 0, entries_scanned: 0, scan_issues: 0, operation_errors: 0 };
let points: {
    time: number;
    entries: number;
    bytes: number;
    scanned: number;
}[] = [];
export function recordSnapshot(snapshot: Snapshot) {
    if (snapshot.active && (!latest.active || snapshot.phase !== latest.phase || snapshot.profile !== latest.profile))
        points = [];
    latest = snapshot;
    if (snapshot.active) {
        points.push({ time: Date.now(), entries: snapshot.entries_done, bytes: snapshot.bytes_done, scanned: snapshot.entries_scanned });
        if (points.length > 240)
            points.shift();
    }
}
function chart(key: "entries" | "bytes" | "scanned", label: string) {
    const max = Math.max(1, ...points.map(p => p[key]));
    const start = points[0]?.time ?? 0;
    const span = Math.max(1, (points.at(-1)?.time ?? start) - start);
    const line = points.map(p => `${20 + (p.time - start) / span * 760},${160 - p[key] / max * 130}`).join(" ");
    return `<section class="card"><h2>${label}</h2><svg viewBox="0 0 800 190" role="img" aria-label="${label}, évolution sur les 240 derniers relevés"><path d="M20 20 V160 H780" fill="none" stroke="#b8c9d6"/><polyline points="${line}" fill="none" stroke="#146656" stroke-width="3"/><text x="25" y="25" fill="#17324e">${max.toLocaleString("fr-FR")}</text><text x="25" y="183" fill="#53697d">${Math.round(span / 1000)} secondes observées</text></svg></section>`;
}
export function renderMonitor(container: HTMLElement, profile?: Profile, preview?: Preview) {
    const s = latest;
    const name = profile && s.profile === profile.id ? profile.name : s.profile;
    container.innerHTML = `<h1>Suivi des transferts</h1><p>${s.active ? `${s.phase === "scanning" ? "Simulation · analyse et empreintes" : "Exécution réelle"} · ${e(name)}` : "Aucune tâche active dans cette fenêtre"}</p><div class="metrics"><div class="metric"><strong>${s.entries_scanned.toLocaleString("fr-FR")}</strong><span>Entrées analysées (deux côtés)</span></div><div class="metric conflict"><strong>${(s.scan_issues + s.operation_errors).toLocaleString("fr-FR")}</strong><span>Anomalies / refus observés</span></div><div class="metric"><strong>${s.entries_done.toLocaleString("fr-FR")}</strong><span>Opérations observées</span></div><div class="metric"><strong>${s.bytes_done.toLocaleString("fr-FR")}</strong><span>Octets validés</span></div>${preview ? `<div class="metric"><strong>${preview.summary.copy + preview.summary.replace}</strong><span>Transferts prévus au dernier plan</span></div><div class="metric delete"><strong>${preview.summary.delete}</strong><span>Suppressions prévues</span></div><div class="metric conflict"><strong>${preview.summary.conflict}</strong><span>Conflits au dernier plan</span></div>` : ""}</div>${chart("scanned", "Entrées analysées au fil du temps")}${chart("entries", "Opérations observées au fil du temps")}${chart("bytes", "Octets validés au fil du temps")}<p>Relevés agrégés toutes les 500 ms, conservés dans cette fenêtre (240 maximum). Les erreurs et refus définitifs sont détaillés dans l’historique. Les tâches de l’agent sont consultables dans l’historique après actualisation.</p>`;
}
