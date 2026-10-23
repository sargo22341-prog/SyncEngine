import { api } from "./api";
import type { Profile, ScheduleState, Trigger } from "./api";
import { escape as e } from "./presentation";

export function renderSchedules(content: HTMLElement, profiles: Profile[], rows: ScheduleState[], state: string, busy: boolean, changed: () => Promise<void>, failed: (error: unknown) => void) {
  content.innerHTML = `<h1>Planification par l’agent</h1>
  <p>Les autorisations sont persistées. L’agent fonctionne sans fenêtre et utilise les mêmes moteurs. Une modification du profil exige une nouvelle autorisation.</p>
  <form id="schedule-form" class="card"><label>Profil <select name="profile" required>${profiles.map(p => `<option value="${e(p.id)}">${e(p.name)}</option>`).join("")}</select></label>
  <label>Déclencheur <select name="kind"><option value="interval">Intervalle (secondes)</option><option value="daily_utc">Chaque jour (UTC)</option></select></label>
  <label>Intervalle en secondes (60 à 31536000) <input name="seconds" type="number" min="60" max="31536000" value="3600" required></label>
  <label>Heure UTC <input name="time" type="time" value="12:00" required></label>
  <label class="check"><input name="skip" type="checkbox"> Ignorer les échéances manquées (sinon : une seule relance, sans rejouer le backlog)</label>
  <label class="check warning"><input name="writes" type="checkbox" required> J’autorise les écritures automatiques pour ce profil sans confirmation dans la fenêtre.</label>
  <label class="check warning"><input name="deletions" type="checkbox"> J’autorise aussi les suppressions protégées si le profil les permet.</label>
  <button type="submit" ${busy || !profiles.length ? "disabled" : ""}>Autoriser / remplacer</button></form>
  <h2>Autorisations et dernières observations</h2>${rows.map(row => `<section class="card"><strong>${e(row.schedule.approved.name)}</strong><p>${e(JSON.stringify(row.schedule.trigger))} · ${row.schedule.allow_deletions ? "Suppressions autorisées" : "Aucune suppression autorisée"}</p><p>${row.running_slot != null ? "En cours ou interrompu : diagnostic requis avant réautorisation." : e(row.result?.code ?? "Jamais exécuté")}</p><button data-remove-schedule="${e(row.schedule.approved.id)}" ${busy || row.running_slot != null ? "disabled" : ""}>Retirer l’autorisation</button></section>`).join("") || "<p>Aucune autorisation.</p>"}
  <h2>Activation native</h2><p>L’enregistrement ci-dessus n’installe pas de tâche système. Depuis le dossier d’installation, exporter les définitions avec le compagnon :</p>
  <pre>syncengine-agent export "${e(state)}" "DOSSIER_EXPORT_NEUF" windows</pre>
  <p>Sous Linux, remplacer <code>windows</code> par <code>linux</code>. Installer ensuite les définitions selon le guide de l’agent. Une fermeture de session ne garantit pas la fin d’une tâche. La fenêtre peut rester ouverte et inactive.</p>`;
  const form = content.querySelector<HTMLFormElement>("#schedule-form")!;
  form.onsubmit = async event => {
    event.preventDefault();
    const data = new FormData(form);
    const [hour, minute] = String(data.get("time")).split(":").map(Number);
    const trigger: Trigger = data.get("kind") === "interval" ? {kind:"interval", seconds:Number(data.get("seconds"))} : {kind:"daily_utc", hour, minute};
    const button = form.querySelector<HTMLButtonElement>("button")!;
    button.disabled = true;
    try { await api.scheduleSave(String(data.get("profile")), trigger, data.has("skip"), data.has("deletions")); await changed(); }
    catch (error) { failed(error); button.disabled = false; }
  };
  content.querySelectorAll<HTMLButtonElement>("[data-remove-schedule]").forEach(button => button.onclick = async () => {
    button.disabled = true;
    try { await api.scheduleRemove(button.dataset.removeSchedule!); await changed(); }
    catch (error) { failed(error); button.disabled = false; }
  });
}
