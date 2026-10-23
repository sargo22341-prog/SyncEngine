// Presentation helpers only: no comparison, synchronization policy or mutations.
export const actions: Record<string, string> = {
    copy: "Copier", replace: "Remplacer", delete: "Supprimer", keep: "Conserver",
    conflict: "Conflit", skip: "Ignorer",
};
export const modes: Record<string, string> = {
    backup: "Sauvegarde", mirror: "Miroir", bidirectional: "Bidirectionnel",
};
export function reasonLabel(code: string): string {
    return ({ missing_at_destination: "Absent de la destination", source_newer: "Source plus récente", destination_newer: "Destination plus récente", content_differs: "Contenu différent", ambiguous_change: "Modification ambiguë", kind_mismatch: "Types différents : conflit à résoudre", equivalent: "Éléments identiques", destination_only: "Présent uniquement à destination", overwrite_disabled: "Remplacement interdit par le profil", deletion_disabled: "Suppression interdite par le profil", absent_from_source: "Absent de la source : suppression prévue", uncertain: "Observation incertaine : aucune mutation", unsupported_entry_kind: "Lien ou type non pris en charge", parent_unavailable: "Dossier parent indisponible", directory_not_emptied: "Dossier conservé : contenu non supprimable", deletion_refused: "Suppression bloquée par un garde-fou", concurrent_change: "Modifications concurrentes : conflit" } as Record<string, string>)[code] ?? code;
}
export function escape(value: unknown): string {
    return String(value ?? "").replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}
export function errorMessage(error: unknown): string {
    if (typeof error === "object" && error && "message" in error) {
        const value = error as {
            message: unknown;
            code?: unknown;
            profile?: unknown;
        };
        return `${value.code ?? "erreur"} : ${value.message}${value.profile ? ` (profil ${value.profile})` : ""}`;
    }
    return String(error);
}
export function timestamp(value?: {
    secs: number;
    nanos: number;
} | null): string {
    return value ? new Date(value.secs * 1000).toLocaleString("fr-FR") : "Inconnue";
}
export function runStatus(run: {
    state: string;
    report?: unknown;
}): string {
    if (run.state === "running")
        return "Interrompu ou en cours — diagnostic requis";
    if (run.state === "error")
        return "Échec — consulter le diagnostic";
    const report = run.report as {
        status?: string;
        execution?: {
            status?: string;
        };
        verification_error?: string;
        deletions_refused?: number;
    } | undefined;
    if (report?.verification_error)
        return "Échec de vérification de la baseline";
    const status = report?.execution?.status ?? report?.status;
    if (status === "succeeded" && report?.deletions_refused)
        return "Miroir incomplet — suppressions refusées";
    return ({ succeeded: "Réussi", succeeded_with_warnings: "Terminé avec avertissements", cancelled: "Annulé — état partiel", failed: "Échec — état partiel" } as Record<string, string>)[status ?? ""] ?? "Résultat à inspecter";
}
