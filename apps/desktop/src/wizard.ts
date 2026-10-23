import { api } from "./api";
import type { Profile } from "./api";
import { escape as e, modes, errorMessage } from "./presentation";
function on(id: string, fn: () => void) { const button = document.getElementById(id); if (button) button.onclick = fn; }
export function openProfileWizard(wizard: HTMLDialogElement, existing: Profile | undefined, saved: () => Promise<void>) {
  const value: Profile = existing ? structuredClone(existing) : { id: `profile-${crypto.randomUUID()}`, name: "", group: "", mode: "backup", source: "", destination: "", allow_delete: false, filter: { include_files: [], exclude_files: [], exclude_directories: [], case_insensitive: false } };
  let step = 0;
  const draw = () => {
    wizard.innerHTML = `<form id="profile-form"><p class="eyebrow">${existing ? "MODIFIER" : "NOUVEAU PROFIL"} · ÉTAPE ${step + 1} / 3</p><h2>${["Identité et mode", "Dossiers locaux", "Règles et vérification"][step]}</h2><div id="wizard-error" role="alert"></div>
    ${step === 0 ? `<label>Nom <input name="name" required maxlength="200" value="${e(value.name)}" autofocus></label><label>Groupe <input name="group" maxlength="200" value="${e(value.group)}"></label><label>Mode <select name="mode">${Object.entries(modes).map(([key,label]) => `<option value="${key}" ${key === value.mode ? "selected" : ""}>${label}</option>`).join("")}</select></label><p>Sauvegarde : source vers destination sans suppression. Miroir : suppressions uniquement si autorisées. Bidirectionnel : état précédent et conflits conservés.</p>` : step === 1 ? `<label>Source · chemin absolu d’un dossier existant <input name="source" required value="${e(value.source)}" spellcheck="false"></label><label>Destination · chemin absolu d’un dossier existant <input name="destination" required value="${e(value.destination)}" spellcheck="false"></label><p>Les dossiers doivent être disjoints entre eux et du répertoire d’état de l’application. Seul le connecteur local est disponible.</p>` : `<p><strong>${e(value.name)}</strong> · ${e(modes[value.mode])}</p><p>${e(value.source)}<br>→ ${e(value.destination)}</p><label>Exclure des fichiers (un glob par ligne)<textarea name="exclude-files">${e(value.filter.exclude_files.join("\n"))}</textarea></label><label>Exclure des dossiers (un glob par ligne)<textarea name="exclude-directories">${e(value.filter.exclude_directories.join("\n"))}</textarea></label><label>Inclure des fichiers (vide = tous, un glob par ligne)<textarea name="include-files">${e(value.filter.include_files.join("\n"))}</textarea></label><label class="check"><input name="case-insensitive" type="checkbox" ${value.filter.case_insensitive ? "checked" : ""}> Filtres insensibles à la casse</label>${value.mode !== "backup" ? `<label class="check warning"><input name="allow-delete" type="checkbox" ${value.allow_delete ? "checked" : ""}> Autoriser les suppressions protégées (confirmation à chaque exécution)</label>` : ""}<p>Écrasement : priorité source ; conflits bidirectionnels préservés. Versions conservées avant écrasement ou suppression, sans purge automatique.</p>`}
    <div class="toolbar"><button id="wizard-cancel" type="button">Annuler</button>${step > 0 ? '<button id="back" type="button">Retour</button>' : ""}<button class="primary" type="submit">${step === 2 ? "Enregistrer" : "Continuer"}</button></div></form>`;
    on("wizard-cancel", () => wizard.close()); on("back", () => { step--; draw(); });
    document.querySelector<HTMLFormElement>("#profile-form")!.onsubmit = async event => {
      event.preventDefault();
      const form = event.target as HTMLFormElement;
      const data = new FormData(form);
      if (step === 0) { value.name = String(data.get("name")); value.group = String(data.get("group")); value.mode = String(data.get("mode")) as Profile["mode"]; if (value.mode === "backup") value.allow_delete = false; }
      if (step === 1) { value.source = String(data.get("source")); value.destination = String(data.get("destination")); }
      if (step < 2) { step++; draw(); return; }
      const lines = (key: string) => String(data.get(key)).split(/\r?\n/).map(s => s.trim()).filter(Boolean);
      value.filter = { include_files: lines("include-files"), exclude_files: lines("exclude-files"), exclude_directories: lines("exclude-directories"), case_insensitive: data.has("case-insensitive") };
      value.allow_delete = value.mode !== "backup" && data.has("allow-delete");
      const submit = form.querySelector<HTMLButtonElement>('button[type="submit"]')!; submit.disabled = true;
      try { await api.save(value); wizard.close(); await saved(); }
      catch (error) { document.getElementById("wizard-error")!.textContent = errorMessage(error); }
      finally { submit.disabled = false; }
    };
  };
  draw(); wizard.showModal();
}
