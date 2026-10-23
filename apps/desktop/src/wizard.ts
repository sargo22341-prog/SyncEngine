import { api } from "./api";
import type { Profile } from "./api";
import { escape as e, modes, errorMessage } from "./presentation";
function on(id: string, fn: () => void) { const button = document.getElementById(id); if (button) button.onclick = fn; }
export async function openProfileWizard(wizard: HTMLDialogElement, existing: Profile | undefined, saved: () => Promise<void>) {
  const value: Profile = existing ? structuredClone(existing) : { id: `profile-${crypto.randomUUID()}`, name: "", group: "", mode: "backup", source: "", destination: "", allow_delete: false, filter: { include_files: [], exclude_files: [], exclude_directories: [], case_insensitive: false } };
  const providers = await api.providers();
  let step = 0;
  const draw = () => {
    wizard.innerHTML = `<form id="profile-form"><p class="eyebrow">${existing ? "MODIFIER" : "NOUVEAU PROFIL"} · ÉTAPE ${step + 1} / 3</p><h2>${["Identité et mode", "Sources et destinations", "Règles et vérification"][step]}</h2><div id="wizard-error" role="alert"></div>
    ${step === 0 ? `<label>Nom <input name="name" required maxlength="200" value="${e(value.name)}" autofocus></label><label>Groupe <input name="group" maxlength="200" value="${e(value.group)}"></label><label>Mode <select name="mode">${Object.entries(modes).map(([key,label]) => `<option value="${key}" ${key === value.mode ? "selected" : ""}>${label}</option>`).join("")}</select></label><p>Sauvegarde : source vers destination sans suppression. Miroir : suppressions uniquement si autorisées. Bidirectionnel : état précédent et conflits conservés.</p>` : step === 1 ? `<label>Source : chemin local (fournisseur local uniquement) <input name="source" value="${e(value.source)}" spellcheck="false"></label><label>Destination : chemin local (fournisseur local uniquement) <input name="destination" value="${e(value.destination)}" spellcheck="false"></label><p>Les racines locales doivent rester disjointes. Les configurations distantes ne contiennent aucun secret. SSH utilise l'agent ; les autres secrets viennent de SYNCENGINE_SECRET_REFERENCE.</p>${remoteEditor("source", value.remote_source, providers)}${remoteEditor("destination", value.remote_destination, providers.filter(p => p !== "onedrive"))}<p>OneDrive expérimental : source en lecture seule, jeton externe, pas de snapshot. Une extrémité locale est requise ; bidirectionnel local uniquement.</p>` : `<p><strong>${e(value.name)}</strong> · ${e(modes[value.mode])}</p><p>${e(value.remote_source ? JSON.stringify(value.remote_source) : value.source)}<br>→ ${e(value.remote_destination ? JSON.stringify(value.remote_destination) : value.destination)}</p><label>Exclure des fichiers (un glob par ligne)<textarea name="exclude-files">${e(value.filter.exclude_files.join("\n"))}</textarea></label><label>Exclure des dossiers (un glob par ligne)<textarea name="exclude-directories">${e(value.filter.exclude_directories.join("\n"))}</textarea></label><label>Inclure des fichiers (vide = tous, un glob par ligne)<textarea name="include-files">${e(value.filter.include_files.join("\n"))}</textarea></label><label class="check"><input name="case-insensitive" type="checkbox" ${value.filter.case_insensitive ? "checked" : ""}> Filtres insensibles à la casse</label>${value.mode !== "backup" ? `<label class="check warning"><input name="allow-delete" type="checkbox" ${value.allow_delete ? "checked" : ""}> Autoriser les suppressions protégées (confirmation à chaque exécution)</label>` : ""}<p>Écrasement : priorité source ; conflits bidirectionnels préservés. Versions conservées avant écrasement ou suppression, sans purge automatique.</p>`}
    <div class="toolbar"><button id="wizard-cancel" type="button">Annuler</button>${step > 0 ? '<button id="back" type="button">Retour</button>' : ""}<button class="primary" type="submit">${step === 2 ? "Enregistrer" : "Continuer"}</button></div></form>`;
    for (const side of ["source", "destination"]) {
      const select = document.querySelector<HTMLSelectElement>(`[name="${side}-provider"]`);
      if (select) select.onchange = () => {
        document.querySelector<HTMLTextAreaElement>(`[name="${side}-config"]`)!.value = JSON.stringify(template(select.value), null, 2);
      };
    }
    on("wizard-cancel", () => wizard.close()); on("back", () => { step--; draw(); });
    document.querySelector<HTMLFormElement>("#profile-form")!.onsubmit = async event => {
      event.preventDefault();
      const form = event.target as HTMLFormElement;
      const data = new FormData(form);
      if (step === 0) { value.name = String(data.get("name")); value.group = String(data.get("group")); value.mode = String(data.get("mode")) as Profile["mode"]; if (value.mode === "backup") value.allow_delete = false; }
      if (step === 1) {
        try {
          value.source = String(data.get("source")); value.destination = String(data.get("destination"));
          for (const side of ["source", "destination"] as const) {
            const provider = String(data.get(`${side}-provider`));
            const remote = provider === "local" ? undefined : JSON.parse(String(data.get(`${side}-config`)));
            if (remote && remote.provider !== provider) throw new Error("Le fournisseur doit correspondre ? la configuration.");
            value[`remote_${side}`] = remote;
          }
        } catch (error) { document.getElementById("wizard-error")!.textContent = errorMessage(error); return; }
      }
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

function template(provider: string) {
  switch (provider) {
    case "sftp": return {provider, address:"127.0.0.1:22", host:"server.example", username:"backup", known_hosts:"CHEMIN_ABSOLU_KNOWN_HOSTS", root:"/backup"};
    case "webdav": return {provider, root:"https://server.example/backup/", credential_ref:"DAV"};
    case "ftp": return {provider, address:"127.0.0.1:21", username:"backup", root:"/backup", credential_ref:"FTP", acknowledge_plaintext:false, exclusive_writers:false};
    case "ftps": return {provider, address:"127.0.0.1:21", tls_domain:"server.example", username:"backup", root:"/backup", credential_ref:"FTPS", root_certificate:null, exclusive_writers:false};
    case "smb": return {provider, root:"CHEMIN_ABSOLU_PARTAGE"};
    case "onedrive": return {provider, drive_id:"DRIVE_ID", download_hosts:[], credential_ref:"ONEDRIVE"};
    default: return {};
  }
}
function remoteEditor(side: string, remote: Profile["remote_source"], providers: string[]) {
  return `<label>${side === "source" ? "Source" : "Destination"} ? fournisseur <select name="${side}-provider">${providers.map(provider => `<option value="${provider}" ${provider === (remote?.provider ?? "local") ? "selected" : ""}>${provider === "onedrive" ? "OneDrive exp?rimental ? lecture seule" : provider}</option>`).join("")}</select></label><label>Configuration distante avanc?e (JSON, aucun mot de passe ni jeton)<textarea name="${side}-config" rows="8" spellcheck="false">${e(JSON.stringify(remote ?? {}, null, 2))}</textarea></label>`;
}
