import { api } from "./api";
import type { Profile } from "./api";
import { escape as e, modes, errorMessage } from "./presentation";
import { endpointEditor, endpointFields, readEndpoint, bindBrowse, endpointLabel } from "./endpoint-editor";
import { scheduleFields, bindScheduleFields, saveSchedule } from "./schedules";
export async function openProfileWizard(container: HTMLElement, existing: Profile | undefined, saved: (id?: string) => Promise<void>, cancelled: () => void) {
    const value: Profile = existing ? structuredClone(existing) : { id: `profile-${crypto.randomUUID()}`, name: "", group: "", mode: "backup", source: "", destination: "", allow_delete: false, filter: { include_files: [], exclude_files: [], exclude_directories: [], case_insensitive: false } };
    const [providers, schedules, profiles] = await Promise.all([api.providers(), api.schedules(), api.profiles()]);
    const schedule = schedules.schedules.find(r => r.schedule.approved.id === value.id);
    let step = 0;
    let scheduleDraft: FormData | undefined;
    const titles = ["Identité et mode", "Source et destination", "Règles et vérification", "Planification"];
    const capture = () => {
        const data = new FormData(container.querySelector<HTMLFormElement>("form")!);
        if (step === 3)
            scheduleDraft = data;
        if (step === 0) {
            value.name = String(data.get("name")).trim();
            value.group = String(data.get("group")).trim();
            value.mode = String(data.get("mode")) as Profile["mode"];
            if (value.mode === "backup")
                value.allow_delete = false;
        }
        if (step === 1)
            for (const side of ["source", "destination"] as const) {
                value[`remote_${side}`] = readEndpoint(data, side);
                if (!value[`remote_${side}`])
                    value[side] = String(data.get(side) ?? "");
            }
        if (step === 2) {
            const lines = (key: string) => String(data.get(key) ?? "").split(/\r?\n/).map(s => s.trim()).filter(Boolean);
            value.filter = { include_files: lines("include-files"), exclude_files: lines("exclude-files"), exclude_directories: lines("exclude-directories"), case_insensitive: data.has("case-insensitive") };
            value.allow_delete = value.mode !== "backup" && data.has("allow-delete");
        }
    };
    const draw = () => {
        container.innerHTML = `<div class="heading"><div><p class="eyebrow">${existing ? "MODIFIER LE PROFIL" : "NOUVEAU PROFIL"}</p><h1>${titles[step]}</h1></div><span>Étape ${step + 1} sur 4</span></div><ol class="steps">${titles.map((title, i) => `<li class="${step === i ? "active" : ""}">${i + 1}. ${title}</li>`).join("")}</ol><form id="profile-form"><div id="wizard-error" role="alert"></div>${step === 0 ? `<section class="card"><label>Nom du profil <input name="name" required maxlength="200" value="${e(value.name)}"></label><label>Groupe <input name="group" list="groups" maxlength="200" value="${e(value.group)}" placeholder="Sans groupe"><datalist id="groups">${[...new Set(profiles.map(p => p.group).filter(Boolean))].map(g => `<option value="${e(g)}">`).join("")}</datalist></label><label>Mode <select name="mode">${Object.entries(modes).map(([key, label]) => `<option value="${key}" ${key === value.mode ? "selected" : ""}>${label}</option>`).join("")}</select></label><p>Sauvegarde : copie sans suppression. Miroir : destination identique à la source pour les éléments inclus, sous réserve des protections. Bidirectionnel : état précédent et conflits conservés.</p></section>` : step === 1 ? `<div class="endpoint-grid">${endpointEditor("source", value.source, value.remote_source, providers)}${endpointEditor("destination", value.destination, value.remote_destination, providers.filter(p => p !== "onedrive"))}</div><p>Les dossiers doivent rester disjoints. Une extrémité locale est requise ; bidirectionnel local uniquement.</p>` : step === 2 ? `<section class="card"><h2>Filtres des fichiers et dossiers</h2><p>Un motif par ligne. Exemples : <code>*.tmp</code> exclut les fichiers temporaires ; <code>**/*.jpg</code> cible les photos dans les sous-dossiers ; <code>cache</code> exclut un dossier nommé cache. Les mêmes filtres s’appliquent aux deux côtés.</p>${[["exclude-files", "Exclure des fichiers", value.filter.exclude_files], ["exclude-directories", "Exclure des dossiers", value.filter.exclude_directories], ["include-files", "Inclure des fichiers (vide = tous)", value.filter.include_files]].map(([key, label, lines]) => `<label>${label}<textarea name="${key}">${e((lines as string[]).join("\n"))}</textarea></label>`).join("")}<label class="check"><input name="case-insensitive" type="checkbox" ${value.filter.case_insensitive ? "checked" : ""}> Filtres insensibles à la casse</label></section><section class="card"><h2>Intégrité et suppressions</h2><p>Comparaison et vérification BLAKE3. Versions préservées avant remplacement ou suppression. Conflits bidirectionnels conservés sans écriture arbitraire.</p>${value.mode !== "backup" ? `<label class="check warning"><input name="allow-delete" type="checkbox" ${value.allow_delete ? "checked" : ""}> Autoriser les suppressions protégées${value.mode === "mirror" ? " pour répliquer les retraits de la source" : ""}</label>` : ""}<p>Source inaccessible, inventaire incomplet, source vide ou plus de 50 % de suppressions : les suppressions restent bloquées. Un miroir sans autorisation conserve les fichiers supplémentaires ; la simulation signale ces refus.</p></section>` : `<section class="card"><h2>${e(value.name)}</h2><p>${e(endpointLabel(value.source, value.remote_source))} · ${e(endpointLabel(value.destination, value.remote_destination))}</p>${scheduleFields(schedule)}</section>`}<div class="toolbar"><button type="button" id="wizard-cancel">Annuler</button>${step > 0 ? '<button type="button" id="back">Retour</button>' : ""}<button class="primary" type="submit">${step === 3 ? "Enregistrer le profil et la planification" : "Continuer"}</button></div></form>`;
        container.querySelector<HTMLButtonElement>("#wizard-cancel")!.onclick = cancelled;
        const back = container.querySelector<HTMLButtonElement>("#back");
        if (back)
            back.onclick = () => { capture(); step--; draw(); };
        if (step === 1) {
            bindBrowse(container);
            for (const side of ["source", "destination"] as const) {
                const select = container.querySelector<HTMLSelectElement>(`[name="${side}-provider"]`)!;
                let previous = select.value;
                const drafts: Record<string, Profile["remote_source"]> = {};
                if (value[`remote_${side}`])
                    drafts[previous] = value[`remote_${side}`];
                select.onchange = () => {
                    const data = new FormData(container.querySelector<HTMLFormElement>("form")!);
                    data.set(`${side}-provider`, previous);
                    if (previous === "local")
                        value[side] = String(data.get(side) ?? "");
                    else
                        drafts[previous] = readEndpoint(data, side);
                    previous = select.value;
                    container.querySelector<HTMLElement>(`#${side}-fields`)!.innerHTML = endpointFields(side, select.value, value[side], drafts[select.value]);
                    bindBrowse(container);
                };
            }
        }
        if (step === 3) {
            if (scheduleDraft)
                container.querySelectorAll<HTMLInputElement | HTMLSelectElement>("input[name],select[name]").forEach(input => { if (input instanceof HTMLInputElement && input.type === "checkbox")
                    input.checked = scheduleDraft!.has(input.name);
                else if (scheduleDraft!.has(input.name))
                    input.value = String(scheduleDraft!.get(input.name)); });
            bindScheduleFields(container);
        }
        container.querySelector<HTMLFormElement>("form")!.onsubmit = async (event) => {
            event.preventDefault();
            capture();
            if (step < 3) {
                step++;
                draw();
                return;
            }
            const form = event.target as HTMLFormElement;
            const data = new FormData(form);
            const button = form.querySelector<HTMLButtonElement>('[type="submit"]')!;
            button.disabled = true;
            let profileSaved = false;
            try {
                await api.save(value);
                profileSaved = true;
                await saveSchedule(value.id, data, value.allow_delete, !!schedule);
                await saved(value.id);
            }
            catch (error) {
                container.querySelector<HTMLElement>("#wizard-error")!.textContent = `${profileSaved ? "Profil enregistré, mais planification non enregistrée : " : ""}${errorMessage(error)}`;
            }
            finally {
                button.disabled = false;
            }
        };
    };
    draw();
}
