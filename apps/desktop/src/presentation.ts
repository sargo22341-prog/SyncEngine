// Presentation helpers only: no comparison, synchronization policy or mutations.
export const actions: Record<string, string> = {
  copy: "Copier", replace: "Remplacer", delete: "Supprimer", keep: "Conserver",
  conflict: "Conflit", skip: "Ignorer",
};
export const modes: Record<string, string> = {
  backup: "Sauvegarde", mirror: "Miroir", bidirectional: "Bidirectionnel",
};
export function escape(value: unknown): string {
  return String(value ?? "").replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}
export function errorMessage(error: unknown): string {
  if (typeof error === "object" && error && "message" in error) {
    const value = error as { message: unknown; code?: unknown; profile?: unknown };
    return `${value.code ?? "erreur"} : ${value.message}${value.profile ? ` (profil ${value.profile})` : ""}`;
  }
  return String(error);
}
export function timestamp(value?: { secs: number; nanos: number } | null): string {
  return value ? new Date(value.secs * 1000).toLocaleString("fr-FR") : "Inconnue";
}
export function runStatus(run: { state: string; report?: unknown }): string {
  if (run.state === "running") return "Interrompu ou en cours — diagnostic requis";
  if (run.state === "error") return "Échec — consulter le diagnostic";
  const report = run.report as { status?: string; execution?: { status?: string }; verification_error?: string } | undefined;
  if (report?.verification_error) return "Échec de vérification de la baseline";
  const status = report?.execution?.status ?? report?.status;
  return ({ succeeded: "Réussi", succeeded_with_warnings: "Terminé avec avertissements", cancelled: "Annulé — état partiel", failed: "Échec — état partiel" } as Record<string, string>)[status ?? ""] ?? "Résultat à inspecter";
}
