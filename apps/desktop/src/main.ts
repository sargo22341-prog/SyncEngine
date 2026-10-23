import { endpointLabel } from "./endpoint-editor";
import { historyCards, historyOperations } from "./history-view";
import { renderMonitor, recordSnapshot } from "./monitor";
import { renderSchedules } from "./schedules";
import type { ScheduleState } from "./api";
import { openProfileWizard } from "./wizard";
import { api } from "./api";
import type { Profile, Preview, Run, Operation, Observation } from "./api";
import { escape as e, actions, modes, errorMessage, timestamp, runStatus, reasonLabel } from "./presentation";
import "./style.css";
const root = document.querySelector<HTMLDivElement>("#app")!;
root.innerHTML = `<header><div class="brand">S<span>SyncEngine</span></div><span class="tag">0.1.0</span></header>
<div class="layout"><nav aria-label="Navigation principale"><button data-view="profiles" class="selected">Profils</button><button data-view="differences">Simulation</button><button data-view="monitor">Suivi des transferts</button><button data-view="history">Historique</button><button data-view="schedules">Planification</button><button data-view="settings">Paramètres</button><p>Vos fichiers.<br>Des décisions visibles.</p><small>Agent autonome · horaires UTC</small></nav>
<main><div id="error" role="alert" hidden></div><div id="notice" role="status" aria-live="polite"></div><section id="content"></section></main></div>
<footer><div id="progress" role="status" aria-live="polite">Prêt · Simulation avant toute exécution</div><button id="cancel" disabled>Annuler la tâche</button></footer><dialog id="confirmation"></dialog>`;
const content = document.querySelector<HTMLElement>("#content")!;
const errorBox = document.querySelector<HTMLElement>("#error")!;
const notice = document.querySelector<HTMLElement>("#notice")!;
const confirmation = document.querySelector<HTMLDialogElement>("#confirmation")!;
let profiles: Profile[] = [], selected = "", view = "profiles", busy = false;
let preview: Preview | undefined;
let search = "", actionFilter = "", page = 0;
let scheduleRows: ScheduleState[] = [], stateDirectory = "";
let startup = false, windowsIntegration = false;
let histories: Record<string, Run[]> = {};
function fail(error: unknown) { errorBox.textContent = errorMessage(error); errorBox.hidden = false; }
function clearError() { errorBox.hidden = true; }
function profile() { return profiles.find(p => p.id === selected); }
function selectView(next: string) {
    view = next;
    document.querySelectorAll<HTMLButtonElement>("[data-view]").forEach(b => b.classList.toggle("selected", b.dataset.view === next));
    render();
}
async function refresh() {
    const schedules = await api.schedules();
    scheduleRows = schedules.schedules;
    stateDirectory = schedules.state;
    profiles = await api.profiles();
    if (!profiles.some(p => p.id === selected))
        selected = profiles[0]?.id ?? "";
    // Fetch actual persisted last results; errors are explicit, never invented.
    const values = await Promise.all(profiles.map(async (p) => [p.id, await api.history(p.id)] as const));
    histories = Object.fromEntries(values);
}
async function task<T>(label: string, fn: () => Promise<T>): Promise<T | undefined> {
    if (busy)
        return;
    busy = true;
    clearError();
    notice.textContent = label;
    render();
    try {
        return await fn();
    }
    catch (error) {
        fail(error);
        return undefined;
    }
    finally {
        busy = false;
        notice.textContent = "";
        render();
    }
}
function render() {
    const p = profile();
    if (view === "profiles") {
        content.innerHTML = `<div class="heading"><div><p class="eyebrow">ESPACE DE TRAVAIL</p><h1>Vos profils</h1><p>Préparez, inspectez, puis exécutez.</p></div><button id="new" class="primary" ${busy ? "disabled" : ""}>+ Nouveau profil</button></div>
    <div class="toolbar"><button id="edit" ${!p || busy ? "disabled" : ""}>Modifier</button><button id="simulate" ${!p || busy ? "disabled" : ""}>Simuler</button><button id="remove" ${!p || busy ? "disabled" : ""}>Retirer le profil</button><label class="search">Rechercher <input id="profiles-search" type="search" placeholder="Nom ou groupe"></label></div>
    <div class="table-wrap"><table><caption>Profils · sélectionnez une ligne ou une action</caption><thead><tr><th>Profil / groupe</th><th>Mode</th><th>Dernier lancement</th><th>Résultat</th><th>Actions</th></tr></thead><tbody>${[...profiles].sort((a, b) => a.group.localeCompare(b.group) || a.name.localeCompare(b.name)).map((row, index, sorted) => {
            const last = histories[row.id]?.[0];
            return `${index === 0 || sorted[index - 1].group !== row.group ? `<tr class="group-row"><th colspan="4">${e(row.group || "Sans groupe")}</th><td>${row.group ? `<button data-group="${e(row.group)}" aria-label="Modifier le groupe ${e(row.group)}">✎ Groupe</button>` : ""}</td></tr>` : ""}<tr data-profile="${e(row.id)}" ${selected === row.id ? 'class="current"' : ""}><td><button class="profile-select" data-id="${e(row.id)}" aria-pressed="${selected === row.id}">${e(row.name)}</button><small>${e(row.group || "Sans groupe")}</small></td><td>${e(modes[row.mode])}</td><td>${last ? e(timestamp(JSON.parse(last.started))) : "Jamais"}</td><td>${last ? e(runStatus(last)) : "Non exécuté"}</td><td class="row-actions">${[["simulate", "▷", "Simuler"], ["edit", "✎", "Modifier"], ["remove", "🗑", "Retirer"]].map(([action, icon, label]) => `<button data-row-action="${action}" data-row-id="${e(row.id)}" title="${label} ${e(row.name)}" aria-label="${label} ${e(row.name)}" ${busy ? "disabled" : ""}>${icon}</button>`).join("")}</td></tr>`;
        }).join("")}</tbody></table>${!profiles.length ? '<div class="empty"><h2>Aucun profil</h2><p>Créez un profil avec deux dossiers locaux existants pour commencer.</p></div>' : ""}</div>
    ${p ? `<aside class="endpoints"><div><small>SOURCE</small><code>${e(endpointLabel(p.source, p.remote_source))}</code></div><span>→</span><div><small>DESTINATION</small><code>${e(endpointLabel(p.destination, p.remote_destination))}</code></div></aside>` : ""}`;
        on("new", () => editProfile());
        on("edit", () => p && editProfile(p));
        on("simulate", () => void simulate());
        on("remove", () => p && removeProfile(p));
        content.querySelectorAll<HTMLTableRowElement>("[data-profile]").forEach(row => { row.onclick = () => { if (busy)
            return; selected = row.dataset.profile!; preview = undefined; render(); }; });
        content.querySelectorAll<HTMLButtonElement>("[data-row-action]").forEach(button => button.onclick = event => { event.stopPropagation(); if (busy)
            return; const item = profiles.find(p => p.id === button.dataset.rowId); if (!item)
            return; selected = item.id; preview = undefined; if (button.dataset.rowAction === "edit")
            editProfile(item);
        else if (button.dataset.rowAction === "simulate")
            void simulate();
        else
            removeProfile(item); });
        content.querySelectorAll<HTMLButtonElement>("[data-group]").forEach(button => button.onclick = () => editGroup(button.dataset.group!));
        document.querySelector<HTMLInputElement>("#profiles-search")!.oninput = event => {
            const query = (event.target as HTMLInputElement).value.toLocaleLowerCase();
            content.querySelectorAll<HTMLTableRowElement>("[data-profile]").forEach(row => row.hidden = !row.textContent!.toLocaleLowerCase().includes(query));
        };
    }
    else if (view === "schedules")
        renderSchedules(content, profiles, scheduleRows, stateDirectory, busy, async () => { await refresh(); render(); }, fail, selected, windowsIntegration);
    else if (view === "differences")
        renderDifferences();
    else if (view === "monitor")
        renderMonitor(content, profile(), preview);
    else if (view === "editor")
        return;
    else if (view === "history") {
        content.innerHTML = `<h1>Historique</h1><label>Profil <select id="history-profile">${profiles.map(p => `<option value="${e(p.id)}" ${p.id === selected ? "selected" : ""}>${e(p.name)}</option>`).join("")}</select></label><button id="reload-history" ${busy ? "disabled" : ""}>Actualiser</button><p>Les états partiels et erreurs restent visibles. Un état en cours après redémarrage exige un diagnostic.</p>${historyCards(histories[selected] ?? [])}`;
        content.querySelectorAll<HTMLButtonElement>("[data-run]").forEach(button => button.onclick = async () => {
            button.disabled = true;
            try {
                const offset = Number(button.dataset.offset);
                const result = await api.historyPage(selected, button.dataset.run!, offset);
                const target = document.getElementById(`run-${button.dataset.run!}`);
                if (target)
                    target.innerHTML = historyOperations(result.operations);
                button.dataset.offset = String(offset + 100 >= result.total ? 0 : offset + 100);
                button.textContent = `${offset + 1}-${Math.min(offset + 100, result.total)} / ${result.total} : page suivante`;
            }
            catch (error) {
                fail(error);
            }
            finally {
                button.disabled = false;
            }
        });
        document.querySelector<HTMLSelectElement>("#history-profile")!.disabled = busy;
        document.querySelector<HTMLSelectElement>("#history-profile")!.onchange = event => { if (busy)
            return; selected = (event.target as HTMLSelectElement).value; preview = undefined; render(); };
        on("reload-history", () => void task("Lecture de l’historique…", refresh));
    }
    else {
        content.innerHTML = `<h1>Paramètres</h1><section class="card"><h2>Démarrage Windows</h2><label class="check"><input id="startup" type="checkbox" ${windowsIntegration ? "" : "disabled"} ${startup ? "checked" : ""}> Ouvrir l’interface à l’ouverture de ma session Windows</label><p>La planification des synchronisations reste indépendante de l’ouverture de l’interface.</p></section><section class="card"><h2>Affichage</h2><label class="check"><input id="compact" type="checkbox" ${document.body.classList.contains("compact") ? "checked" : ""}> Tableaux compacts</label></section><section class="card"><h2>Garanties d’exécution</h2><p>Empreintes BLAKE3 et vérification du contenu. Conservation des versions avant remplacement ou suppression.</p><p>Suppressions désactivées à la création. Si autorisées : source vide refusée et plafond de 50 % conservé. Confirmation distincte à chaque exécution.</p><p>Un verrou par compte sérialise les exécutions CLI, GUI, agent et exemple. Une fenêtre inactive laisse travailler l'agent.</p><p>Fermer pendant une tâche demande son annulation ; la fenêtre reste ouverte jusqu’à la fin. Fermez-la ensuite.</p></section><section class="card"><h2>Capacités disponibles</h2><p>Dossiers locaux · sauvegarde · miroir · bidirectionnel.</p><p>Planification par agent autonome avec autorisation explicite. Connecteurs distants selon les features du build ; OneDrive expérimental en source seule. Pause et restauration guidée indisponibles.</p></section>`;
        document.querySelector<HTMLInputElement>("#startup")!.onchange = event => { const checked = (event.target as HTMLInputElement).checked; void task("Démarrage…", async () => { await api.startupSave(checked); startup = checked; }); };
        document.querySelector<HTMLInputElement>("#compact")!.onchange = event => {
            const compact = (event.target as HTMLInputElement).checked;
            void task("Enregistrement…", async () => { await api.savePreferences(compact); document.body.classList.toggle("compact", compact); });
        };
    }
}
function on(id: string, fn: () => void) { const button = document.getElementById(id); if (button)
    button.onclick = fn; }
function observation(value?: Observation) {
    return value ? `${e(value.kind)} · ${e(value.metadata.size ?? "?")} o<br><small>${e(timestamp(value.metadata.modified))}</small>` : "Absence observée / non observé (voir motif)";
}
function renderDifferences() {
    const p = profile();
    if (!preview) {
        content.innerHTML = `<h1>Simulation</h1>${profilePicker()}<div class="empty"><h2>Inspectez avant d’écrire</h2><p>${p ? e(p.name) : "Sélectionnez ou créez un profil."}</p><button id="simulate" class="primary" ${!p || busy ? "disabled" : ""}>Lancer une simulation</button></div>`;
        bindProfilePicker();
        on("simulate", () => void simulate());
        return;
    }
    const { plan, summary } = preview;
    content.innerHTML = `<div class="heading"><div><p class="eyebrow">SIMULATION · AUCUN FICHIER MODIFIÉ</p><h1>Différences</h1>${profilePicker()}<p>${e(p?.name)} · ${e(modes[plan.mode])}</p></div><button id="rescan" ${busy ? "disabled" : ""}>Recalculer</button></div>
  <aside class="endpoints"><div><small>SOURCE</small><code>${e(plan.source.location)}</code></div><span>⇄</span><div><small>DESTINATION</small><code>${e(plan.destination.location)}</code></div></aside>
  <div class="metrics">${["copy", "replace", "delete", "conflict", "skip"].map(action => `<div class="metric ${action}"><strong>${e(summary[action])}</strong><span>${e(actions[action])}</span></div>`).join("")}</div>
  ${!plan.source.complete || !plan.destination.complete ? '<p class="warning">Inventaire incomplet : les chemins incertains sont ignorés et les suppressions refusées.</p>' : ""}
  ${summary.deletions_refused ? `<p class="warning">${e(summary.deletions_refused)} suppression(s) refusée(s). ${p?.allow_delete ? "Source vide, seuil de 50 %, scan incomplet ou absence incertaine : consultez les motifs." : "Ce profil interdit les suppressions. Autorisez-les dans Règles et vérification puis recalculez."} Le miroir ne sera pas identique tant que ces refus subsistent.</p>` : ""}
  <p>Conflits préservés en place. ${e(summary.bytes)} octets à transférer. Les observations source/destination de chaque ligne représentent l’émetteur et le receveur ; la cible physique est indiquée.</p>
  <div class="toolbar"><label>Rechercher <input id="search" type="search" value="${e(search)}"></label><label>Action <select id="action"><option value="">Toutes</option>${Object.entries(actions).map(([key, label]) => `<option value="${key}" ${key === actionFilter ? "selected" : ""}>${label}</option>`).join("")}</select></label><button id="apply" class="danger" ${busy ? "disabled" : ""}>Exécuter ce plan…</button></div>
  <div id="operations"></div>`;
    bindProfilePicker();
    renderOperations();
    document.querySelector<HTMLInputElement>("#search")!.oninput = event => { search = (event.target as HTMLInputElement).value; page = 0; renderOperations(); };
    document.querySelector<HTMLSelectElement>("#action")!.onchange = event => { actionFilter = (event.target as HTMLSelectElement).value; page = 0; renderOperations(); };
    on("rescan", () => void simulate());
    on("apply", () => ask(`Exécuter « ${p?.name} » ?`, `${summary.copy} copie(s), ${summary.replace} remplacement(s), ${summary.delete} suppression(s). Source : ${plan.source.location}. Destination : ${plan.destination.location}. Les préconditions seront revérifiées ; un échec peut laisser un état partiel.`, summary.delete > 0, async (confirmed) => {
        const exact = preview;
        preview = undefined;
        await task("Exécution réelle…", async () => {
            if (!exact)
                throw new Error("Nouvelle simulation requise");
            const result = await api.execute(exact.plan.profile, exact.plan.id, confirmed);
            await refresh();
            selectView("history");
            // The full result is preserved in history, including failure/warning status.
            notice.textContent = runStatus({ state: "finished", report: result });
        });
    }));
}
let pageGeneration = 0;
function renderOperations() { void loadOperations().catch(fail); }
async function loadOperations() {
    if (!preview)
        return;
    const generation = ++pageGeneration;
    const result = await api.planPage(preview.plan.id, page * 100, search, actionFilter);
    if (generation !== pageGeneration || !document.getElementById("operations"))
        return;
    const operations = { length: result.total };
    const rows = result.operations;
    document.getElementById("operations")!.innerHTML = `<div class="table-wrap"><table><caption>${operations.length} ligne(s) · page ${page + 1} · 100 lignes maximum par page</caption><thead><tr><th>Chemin / motif</th><th>Émetteur observé</th><th>Receveur observé</th><th>Action / cible</th></tr></thead><tbody>${rows.map((op: Operation) => `<tr><td><strong>${e(op.path)}</strong><small>${e(reasonLabel(op.reason.code))}</small></td><td>${observation(op.source)}</td><td>${observation(op.destination)}</td><td><span class="badge ${e(op.action)}">${e(actions[op.action] ?? op.action)}</span><small>${op.target === "source" ? "Source physique" : "Destination physique"}</small></td></tr>`).join("")}</tbody></table>${!rows.length ? '<p class="empty">Aucune différence pour ce filtre.</p>' : ""}</div><div class="pager"><button id="previous" ${page === 0 ? "disabled" : ""}>Précédent</button><button id="next" ${(page + 1) * 100 >= operations.length ? "disabled" : ""}>Suivant</button></div>`;
    on("previous", () => { page--; renderOperations(); });
    on("next", () => { page++; renderOperations(); });
}
async function simulate() {
    if (!selected || busy)
        return;
    preview = undefined;
    search = "";
    actionFilter = "";
    page = 0;
    await task("Simulation : lecture et empreintes…", async () => { preview = await api.preview(selected); selectView("differences"); });
}
function ask(title: string, message: string, deletion: boolean, accept: (deletion: boolean) => unknown) {
    confirmation.innerHTML = `<form method="dialog"><h2>${e(title)}</h2><p>${e(message)}</p>${deletion ? '<label class="check warning"><input id="confirm-delete" type="checkbox" required> Je confirme les suppressions affichées dans ce plan.</label>' : ""}<div class="toolbar"><button value="cancel" formnovalidate>Annuler</button><button value="accept" class="danger">Confirmer</button></div></form>`;
    confirmation.onclose = () => { if (confirmation.returnValue === "accept")
        accept(deletion); };
    confirmation.showModal();
}
function editProfile(existing?: Profile) {
    view = "editor";
    content.innerHTML = "<p>Chargement de l’éditeur…</p>";
    void openProfileWizard(content, existing, async (id) => { preview = undefined; await refresh(); if (id)
        selected = id; selectView("profiles"); }, () => selectView("profiles")).catch(error => { fail(error); selectView("profiles"); });
}
function removeProfile(p: Profile) { ask(`Retirer « ${p.name} » ?`, "Les fichiers et historiques seront conservés. Retirez sa planification avant de le supprimer.", false, () => task("Suppression du profil…", async () => { if (scheduleRows.some(r => r.schedule.approved.id === p.id))
    await api.scheduleRemove(p.id); await api.remove(p.id); preview = undefined; await refresh(); })); }
function profilePicker() { return `<label>Profil à simuler <select id="simulation-profile" ${busy ? "disabled" : ""}>${profiles.map(p => `<option value="${e(p.id)}" ${selected === p.id ? "selected" : ""}>${e(p.name)}</option>`).join("")}</select></label>`; }
function bindProfilePicker() { const select = content.querySelector<HTMLSelectElement>("#simulation-profile"); if (select)
    select.onchange = () => { selected = select.value; preview = undefined; render(); }; }
function editGroup(group: string) {
    confirmation.innerHTML = `<form id="group-form"><h2>Modifier le groupe</h2><div role="alert"></div><label>Nom <input name="group" required maxlength="200" value="${e(group)}"></label><p>Les profils restent dans le même groupe après le renommage. Leurs autorisations programmées sont renouvelées avec leurs réglages existants.</p><div class="toolbar"><button type="button" id="group-cancel">Annuler</button><button type="submit">Renommer</button><button type="button" id="group-schedule">Planifier le groupe</button></div></form>`;
    on("group-cancel", () => confirmation.close());
    on("group-schedule", () => { confirmation.close(); selectView("schedules"); const select = content.querySelector<HTMLSelectElement>('[name="target"]'); if (select) {
        select.value = `group:${group}`;
        select.dispatchEvent(new Event("change"));
    } });
    confirmation.querySelector<HTMLFormElement>("form")!.onsubmit = async (event) => { event.preventDefault(); const form = event.target as HTMLFormElement; const name = String(new FormData(form).get("group")).trim(); if (!name)
        return; const completed: string[] = []; try {
        for (const p of profiles.filter(p => p.group === group)) {
            const row = scheduleRows.find(r => r.schedule.approved.id === p.id);
            if (row?.running_slot != null)
                throw new Error("Tâche en cours ou interrompue : diagnostic requis.");
            await api.save({ ...p, group: name });
            completed.push(p.name);
            if (row)
                await api.scheduleSave(p.id, row.schedule.trigger, row.schedule.missed === "skip", row.schedule.allow_deletions);
        }
        await refresh();
        confirmation.close();
        render();
    }
    catch (error) {
        form.querySelector<HTMLElement>('[role="alert"]')!.textContent = `${errorMessage(error)}. Profils renommés : ${completed.join(", ") || "aucun"}.`;
        await refresh();
    } };
    confirmation.showModal();
}
document.querySelectorAll<HTMLButtonElement>("[data-view]").forEach(button => button.onclick = () => selectView(button.dataset.view!));
on("cancel", () => { void api.cancel().catch(fail); notice.textContent = "Annulation demandée ; attente de la fin de l’opération…"; });
document.addEventListener("keydown", event => {
    if (event.altKey && event.key.toLowerCase() === "n" && !busy && view !== "editor" && !confirmation.open) {
        event.preventDefault();
        editProfile();
    }
    if (event.altKey && event.key.toLowerCase() === "s" && view !== "editor" && !confirmation.open) {
        event.preventDefault();
        void simulate();
    }
});
async function poll() {
    try {
        const s = await api.snapshot();
        recordSnapshot(s);
        if (view === "monitor")
            renderMonitor(content, profile(), preview);
        document.getElementById("progress")!.textContent = s.active ? `${s.phase === "scanning" ? "Simulation · lecture et empreintes" : "EXÉCUTION RÉELLE"} · ${s.entries_scanned} entrées analysées · ${s.entries_done} opérations observées · ${s.bytes_done} octets validés${s.profile ? ` · ${s.profile}` : ""}` : "Prêt · Alt+N : nouveau profil · Alt+S : simuler";
        (document.getElementById("cancel") as HTMLButtonElement).disabled = !s.active;
    }
    catch (error) {
        fail(error);
    }
    window.setTimeout(() => void poll(), 500);
}
render();
void task("Chargement des profils…", async () => { const prefs = await api.preferences(); document.body.classList.toggle("compact", prefs.compact); await refresh(); windowsIntegration = (await api.nativeCapabilities()).windows; if (windowsIntegration)
    startup = await api.startup(); });
void poll();
