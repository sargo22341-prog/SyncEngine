import type { RemoteEndpoint } from "./api.ts";
import { api } from "./api.ts";
import { escape as e, errorMessage } from "./presentation.ts";
export const providerNames: Record<string, string> = { local: "Dossier local", smb: "Partage réseau SMB", sftp: "SFTP / SSH", webdav: "WebDAV HTTPS", ftp: "FTP (non chiffré)", ftps: "FTPS (TLS explicite)", onedrive: "OneDrive · source expérimentale" };
const fields: Record<string, [
    string,
    string,
    string
][]> = {
    sftp: [["address", "Adresse IP et port", "192.168.1.10:22"], ["host", "Nom de l’hôte vérifié", "nas.example"], ["username", "Utilisateur SSH", "backup"], ["known_hosts", "Fichier known_hosts (chemin absolu)", "C:/Users/moi/.ssh/known_hosts"], ["root", "Dossier distant", "/backup"]],
    webdav: [["root", "URL de la collection HTTPS", "https://nas.example/dav/backup/"], ["credential_ref", "Référence du jeton externe", "DAV"]],
    ftp: [["address", "Adresse IP et port", "192.168.1.10:21"], ["username", "Utilisateur", "backup"], ["root", "Dossier distant", "/backup"], ["credential_ref", "Référence du mot de passe externe", "FTP"]],
    ftps: [["address", "Adresse IP et port", "192.168.1.10:21"], ["tls_domain", "Nom vérifié du certificat TLS", "nas.example"], ["username", "Utilisateur", "backup"], ["root", "Dossier distant", "/backup"], ["credential_ref", "Référence du mot de passe externe", "FTPS"], ["root_certificate", "Certificat CA privé (facultatif)", ""]],
    smb: [["root", "Chemin absolu du partage monté / UNC", "\\\\nas\\backup"]],
    onedrive: [["drive_id", "Identifiant du lecteur", ""], ["download_hosts", "Hôtes de téléchargement approuvés (un par ligne)", ""], ["credential_ref", "Référence du jeton externe", "ONEDRIVE"]],
};
export function endpointLabel(local: string, remote?: RemoteEndpoint): string {
    return remote ? `${providerNames[remote.provider] ?? remote.provider} · ${remote.root ?? remote.host ?? remote.drive_id ?? ""}` : local;
}
export function endpointEditor(side: string, local: string, remote: RemoteEndpoint | undefined, providers: string[]) {
    const provider = remote?.provider ?? "local";
    const available = [...new Set(["local", ...providers, provider])];
    return `<section class="card"><h2>${side === "source" ? "Source · fichiers de référence" : "Destination · copie à maintenir"}</h2><label>Emplacement <select name="${side}-provider">${available.map(p => `<option value="${e(p)}" ${provider === p ? "selected" : ""}>${e(providerNames[p] ?? p)}</option>`).join("")}</select></label><div id="${side}-fields">${endpointFields(side, provider, local, remote)}</div></section>`;
}
export function endpointFields(side: string, provider: string, local: string, remote?: RemoteEndpoint) {
    if (provider === "local")
        return `<label>Dossier existant <input name="${side}" required value="${e(local)}" placeholder="Chemin absolu du dossier" spellcheck="false"></label><button type="button" data-browse="${side}">📁 Parcourir les dossiers…</button>`;
    return (fields[provider] ?? []).map(([key, label, placeholder]) => `<label>${e(label)} ${key === "download_hosts" ? `<textarea name="${side}-${key}">${e((remote?.[key] as string[] ?? []).join("\n"))}</textarea>` : `<input name="${side}-${key}" ${key === "root_certificate" ? "" : "required"} value="${e(remote?.[key])}" placeholder="${e(placeholder)}" spellcheck="false">`}</label>`).join("") +
        (["ftp", "ftps"].includes(provider) ? `<label class="check warning"><input type="checkbox" name="${side}-exclusive_writers" ${remote?.exclusive_writers ? "checked" : ""}> Les autres logiciels ne modifient pas ce dossier pendant les transferts.</label>` : "") +
        (provider === "ftp" ? `<label class="check warning"><input type="checkbox" name="${side}-acknowledge_plaintext" required ${remote?.acknowledge_plaintext ? "checked" : ""}> J’accepte que le FTP transmette les identifiants et fichiers sans chiffrement.</label>` : "") +
        `<p class="hint">${provider === "sftp" ? "Authentification via l’agent SSH ; la clé de l’hôte doit déjà être approuvée dans known_hosts." : provider === "smb" ? "Authentification gérée par le système. Linux : lecture seule." : "La référence désigne une variable SYNCENGINE_SECRET_ suivie de ce nom. Aucun secret n’est enregistré dans le profil."}</p>`;
}
export function readEndpoint(data: FormData, side: string): RemoteEndpoint | undefined {
    const provider = String(data.get(`${side}-provider`));
    if (provider === "local")
        return undefined;
    const result: RemoteEndpoint = { provider };
    for (const [key] of fields[provider] ?? []) {
        const value = String(data.get(`${side}-${key}`) ?? "").trim();
        result[key] = key === "download_hosts" ? value.split(/\r?\n/).map(s => s.trim()).filter(Boolean) : key === "root_certificate" && !value ? null : value;
    }
    if (["ftp", "ftps"].includes(provider))
        result.exclusive_writers = data.has(`${side}-exclusive_writers`);
    if (provider === "ftp")
        result.acknowledge_plaintext = data.has(`${side}-acknowledge_plaintext`);
    return result;
}
export function bindBrowse(container: HTMLElement) {
    container.querySelectorAll<HTMLButtonElement>("[data-browse]").forEach(button => button.onclick = async () => {
        button.disabled = true;
        try {
            const path = await api.browseFolder();
            if (path)
                container.querySelector<HTMLInputElement>(`[name="${button.dataset.browse}"]`)!.value = path;
        }
        catch (error) {
            const target = container.querySelector<HTMLElement>("[role=alert]");
            if (target)
                target.textContent = errorMessage(error);
        }
        finally {
            button.disabled = false;
        }
    });
}
