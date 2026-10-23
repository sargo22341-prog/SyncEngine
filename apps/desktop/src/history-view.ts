import type { Run, Operation } from "./api.ts";
import { escape as e, timestamp, runStatus, actions } from "./presentation.ts";
export function historyCards(runs: Run[]) {
    return runs.map(run => {
        const report = run.report as {
            totals?: Record<string, number>;
            message?: string;
            code?: string;
            verification_error?: string;
        } | undefined;
        const labels: Record<string, string> = { done: "Opérations réalisées", no_action: "Sans mutation", refused: "Refusées", failed: "En Échec", cancelled: "Annulées", not_started: "Non commencées", bytes: "Octets vérifiés" };
        return `<details class="card"><summary><strong>${e(timestamp(JSON.parse(run.started)))}</strong> · ${e(runStatus(run))}</summary><p>Exécution : ${e(run.execution)}</p><div class="metrics">${Object.entries(report?.totals ?? {}).map(([key, value]) => `<div class="metric"><strong>${e(value)}</strong><span>${e(labels[key] ?? key)}</span></div>`).join("")}</div>${report?.message || report?.code || report?.verification_error ? `<p class="warning">${e(report.message ?? report.verification_error ?? report.code)}</p>` : ""}<button data-run="${e(run.execution)}" data-offset="0">Afficher les opérations (100 par page)</button><div id="run-${e(run.execution)}"></div></details>`;
    }).join("") || '<div class="empty">Aucune exécution enregistrée pour ce profil.</div>';
}
export function historyOperations(rows: {
    operation: Operation;
    outcome?: {
        status: {
            status: string;
            bytes?: number;
            version?: string;
            error?: {
                message?: string;
            };
            refusal?: {
                cause: string;
            };
        };
    };
}[]) {
    const labels: Record<string, string> = { done: "Réalisée", no_action: "Aucune mutation", refused: "Refusée", failed: "Échec", cancelled: "Annulée", not_started: "Non commencée" };
    return `<div class="table-wrap"><table><thead><tr><th>Chemin</th><th>Action</th><th>Résultat</th><th>Détail</th></tr></thead><tbody>${rows.map(row => { const status = row.outcome?.status; return `<tr><td>${e(row.operation.path)}</td><td>${e(actions[row.operation.action])}</td><td>${e(labels[status?.status ?? ""] ?? "Résultat non enregistré")}</td><td>${e(status?.error?.message ?? status?.refusal?.cause ?? (status?.bytes != null ? `${status.bytes} octets vérifiés` : row.operation.reason.code))}${status?.version ? `<small>Version préservée : ${e(status.version)}</small>` : ""}</td></tr>`; }).join("")}</tbody></table></div>`;
}
