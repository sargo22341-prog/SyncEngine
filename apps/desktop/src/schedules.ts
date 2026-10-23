import { api } from "./api.ts";
import type { Profile, ScheduleState, Trigger } from "./api.ts";
import { escape as e, errorMessage } from "./presentation.ts";
export function triggerLabel(trigger?: Trigger) {
    if (!trigger)
        return "Manuellement · aucune planification";
    switch (trigger.kind) {
        case "interval": return `Toutes les ${trigger.seconds} secondes`;
        case "daily_utc": return `Chaque jour à ${String(trigger.hour).padStart(2, "0")}:${String(trigger.minute).padStart(2, "0")} UTC`;
        case "logon": return "à l’ouverture de session Windows";
        case "logon_delayed": return `à l’ouverture de session Windows + ${trigger.seconds} secondes`;
        case "user_manager_start": return "Au démarrage du gestionnaire utilisateur Linux";
    }
}
export function scheduleFields(row?: ScheduleState) {
    const t = row?.schedule.trigger;
    return `<label>Quand lancer la synchronisation ? <select name="schedule-kind"><option value="none">Aucune · lancement manuel</option>${[["interval", "À intervalle régulier"], ["daily_utc", "Chaque jour (heure UTC)"], ["logon", "à l’ouverture de session Windows"], ["logon_delayed", "à l’ouverture de session Windows avec délai"], ["user_manager_start", "Au démarrage du gestionnaire utilisateur Linux"]].map(([key, label]) => `<option value="${key}" ${t?.kind === key ? "selected" : ""}>${label}</option>`).join("")}</select></label>
  <label data-schedule-field="interval">Intervalle en secondes <input name="seconds" type="number" min="60" max="31536000" value="${t?.kind === "interval" ? t.seconds : 3600}"></label>
  <label data-schedule-field="daily_utc">Heure UTC <input name="time" type="time" value="${t?.kind === "daily_utc" ? `${String(t.hour).padStart(2, "0")}:${String(t.minute).padStart(2, "0")}` : "12:00"}"></label>
  <label data-schedule-field="logon_delayed">Attendre après l’ouverture de session (secondes, 1 à 86400) <input name="delay" type="number" min="1" max="86400" value="${t?.kind === "logon_delayed" ? t.seconds : 60}"></label>
  <div data-schedule-consent><label class="check"><input name="skip" type="checkbox" ${row?.schedule.missed === "skip" ? "checked" : ""}> Ignorer les échéances manquées</label><label class="check warning"><input name="writes" type="checkbox"> J’autorise les écritures automatiques sans confirmation dans la fenêtre.</label><label class="check warning"><input name="deletions" type="checkbox"> J’autorise aussi les suppressions protégées pour les profils qui les permettent.</label></div>
  <p class="hint">L’agent doit être activé dans le système pour déclencher ces tâches. Windows : ouverture de session utilisateur, sans droits administrateur. Un délai est déclenché au prochain passage de l’agent après son échéance.</p>`;
}
export function bindScheduleFields(container: HTMLElement) {
    const select = container.querySelector<HTMLSelectElement>('[name="schedule-kind"]')!;
    const update = () => {
        container.querySelectorAll<HTMLElement>("[data-schedule-field]").forEach(el => {
            el.hidden = el.dataset.scheduleField !== select.value;
            el.querySelectorAll<HTMLInputElement>("input").forEach(input => { input.disabled = el.hidden; input.required = !el.hidden; });
        });
        const consent = container.querySelector<HTMLElement>("[data-schedule-consent]")!;
        consent.hidden = select.value === "none";
        consent.querySelector<HTMLInputElement>('[name="writes"]')!.required = !consent.hidden;
    };
    select.onchange = update;
    update();
}
export function readSchedule(data: FormData): Trigger | undefined {
    switch (data.get("schedule-kind")) {
        case "interval": return { kind: "interval", seconds: Number(data.get("seconds")) };
        case "daily_utc": {
            const [hour, minute] = String(data.get("time")).split(":").map(Number);
            return { kind: "daily_utc", hour, minute };
        }
        case "logon": return { kind: "logon" };
        case "logon_delayed": return { kind: "logon_delayed", seconds: Number(data.get("delay")) };
        case "user_manager_start": return { kind: "user_manager_start" };
        default: return undefined;
    }
}
export async function saveSchedule(id: string, data: FormData, allowDelete: boolean, hadSchedule: boolean) {
    const trigger = readSchedule(data);
    if (trigger) {
        if (!data.has("writes"))
            throw new Error("Autorisation des écritures automatiques requise.");
        await api.scheduleSave(id, trigger, data.has("skip"), allowDelete && data.has("deletions"));
    }
    else if (hadSchedule)
        await api.scheduleRemove(id);
}
export function renderSchedules(content: HTMLElement, profiles: Profile[], rows: ScheduleState[], state: string, busy: boolean, changed: () => Promise<void>, failed: (error: unknown) => void, selected?: string, windows = false) {
    content.innerHTML = `<h1>Planification par l’agent</h1><p>Planifiez un profil ou appliquez les mêmes horaires à tous les membres d’un groupe.</p><form id="schedule-form" class="card"><div role="alert"></div><label>Profil ou groupe <select name="target" required>${profiles.map(p => `<option value="${e(p.id)}" ${selected === p.id ? "selected" : ""}>${e(p.name)}</option>`).join("")}${[...new Set(profiles.map(p => p.group).filter(Boolean))].map(g => `<option value="group:${e(g)}">Groupe : ${e(g)}</option>`).join("")}</select></label><div id="schedule-fields"></div><button type="submit" ${busy || !profiles.length ? "disabled" : ""}>Enregistrer la planification</button></form>
  <h2>Tâches enregistrées</h2>${rows.map(row => `<section class="card"><h3>${e(row.schedule.approved.name)}</h3><p>${e(triggerLabel(row.schedule.trigger))} · ${row.schedule.allow_deletions ? "Suppressions protégées autorisées" : "Suppressions non autorisées"}</p><p>${profiles.some(p => p.id === row.schedule.approved.id && JSON.stringify(p) === JSON.stringify(row.schedule.approved)) ? "" : "Profil modifié ou retiré : autorisation à renouveler. "}${row.running_slot != null ? "En cours ou interrompu : diagnostic requis." : e(row.result?.code ?? "Jamais exécuté")}</p><button data-edit-schedule="${e(row.schedule.approved.id)}">Modifier</button> <button data-remove-schedule="${e(row.schedule.approved.id)}" ${busy || row.running_slot != null ? "disabled" : ""}>Retirer</button></section>`).join("") || "<p>Aucune tâche programmée.</p>"}
  <section class="card"><h2>Activation de l’agent</h2><p>Les horaires sont enregistrés séparément de l’activation système. L’agent fonctionne même lorsque cette fenêtre est fermée.</p>${windows ? '<button type="button" id="activate-agent">Activer / réparer les tâches Windows</button><p>Installe les tâches SyncEngine pour votre compte, sans droits administrateur. Une tâche vérifie les horaires chaque minute ; l’autre signale l’ouverture de session.</p>' : '<p>Linux : activer les unités systemd utilisateur selon le guide de l’agent. Le gestionnaire utilisateur ne correspond pas à chaque connexion au bureau.</p>'}<p class="hint">État de l’application : ${e(state)}. Les changements de profils exigent une nouvelle autorisation.</p><div id="agent-result" role="status"></div></section>`;
    const activate = content.querySelector<HTMLButtonElement>("#activate-agent");
    if (activate)
        activate.onclick = async () => { activate.disabled = true; try {
            await api.agentActivate();
            content.querySelector<HTMLElement>("#agent-result")!.textContent = "Les tâches Windows sont installées. Les prochaines synchronisations utilisent les autorisations enregistrées.";
        }
        catch (error) {
            failed(error);
        }
        finally {
            activate.disabled = false;
        } };
    const form = content.querySelector<HTMLFormElement>("#schedule-form")!;
    const target = form.querySelector<HTMLSelectElement>('[name="target"]')!;
    const draw = () => { const fields = form.querySelector<HTMLElement>("#schedule-fields")!; fields.innerHTML = scheduleFields(rows.find(r => r.schedule.approved.id === target.value)); bindScheduleFields(fields); };
    target.onchange = draw;
    draw();
    form.onsubmit = async (event) => {
        event.preventDefault();
        const data = new FormData(form);
        const button = form.querySelector<HTMLButtonElement>('[type="submit"]')!;
        button.disabled = true;
        const group = target.value.startsWith("group:") ? target.value.slice(6) : undefined;
        const targets = profiles.filter(p => group !== undefined ? p.group === group : p.id === target.value);
        const completed: string[] = [];
        try {
            for (const p of targets) {
                await saveSchedule(p.id, data, p.allow_delete, rows.some(r => r.schedule.approved.id === p.id));
                completed.push(p.name);
            }
            await changed();
        }
        catch (error) {
            failed(new Error(`${errorMessage(error)}. Profils enregistrés : ${completed.join(", ") || "aucun"}.`));
        }
        finally {
            button.disabled = false;
        }
    };
    content.querySelectorAll<HTMLButtonElement>("[data-edit-schedule]").forEach(b => b.onclick = () => { target.value = b.dataset.editSchedule!; draw(); form.scrollIntoView(); });
    content.querySelectorAll<HTMLButtonElement>("[data-remove-schedule]").forEach(b => b.onclick = async () => { b.disabled = true; try {
        await api.scheduleRemove(b.dataset.removeSchedule!);
        await changed();
    }
    catch (error) {
        failed(error);
        b.disabled = false;
    } });
}
