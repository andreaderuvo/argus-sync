//! The few words the Rust side says itself — the tray's menu and tooltip — in the system's
//! language. Everything else is said by the window (ui/words.js); errors travel to it as
//! `{"say": template, "values": {…}}` (see `say`) so it can put them in the same language.

use std::collections::BTreeMap;

fn lang() -> String {
    sys_locale::get_locale().unwrap_or_default().chars().take(2).collect::<String>().to_lowercase()
}

pub fn tr(key: &str) -> String {
    let table: &[(&str, &str, &str, &str)] = &[
        // English, Italiano, Español, Français
        ("Open argus-sync", "Apri argus-sync", "Abrir argus-sync", "Ouvrir argus-sync"),
        ("Sync now", "Sincronizza ora", "Sincronizar ahora", "Synchroniser maintenant"),
        ("Quit", "Esci", "Salir", "Quitter"),
        ("argus-sync — nothing connected", "argus-sync — nessuna cartella collegata", "argus-sync — ninguna carpeta conectada", "argus-sync — aucun dossier connecté"),
        ("argus-sync — paused", "argus-sync — in pausa", "argus-sync — en pausa", "argus-sync — en pause"),
        ("argus-sync — {n} conflict(s) to look at", "argus-sync — {n} conflitti da guardare", "argus-sync — {n} conflictos por revisar", "argus-sync — {n} conflit(s) à regarder"),
        ("argus-sync — {n} folder(s) cannot reach the server", "argus-sync — {n} cartelle non raggiungono il server", "argus-sync — {n} carpetas no llegan al servidor", "argus-sync — {n} dossier(s) n’atteignent pas le serveur"),
        ("argus-sync — syncing…", "argus-sync — sincronizzazione…", "argus-sync — sincronizando…", "argus-sync — synchronisation…"),
        ("argus-sync — up to date", "argus-sync — tutto aggiornato", "argus-sync — todo al día", "argus-sync — à jour"),
    ];
    let col = match lang().as_str() { "it" => 1, "es" => 2, "fr" => 3, _ => 0 };
    table.iter().find(|row| row.0 == key).map(|row| [row.0, row.1, row.2, row.3][col]).unwrap_or(key).to_string()
}

pub fn tr_n(key: &str, n: usize) -> String {
    tr(key).replace("{n}", &n.to_string())
}

/// A message for the window to say in the person's language: the English template is the key.
pub fn say(template: &str, values: &[(&str, String)]) -> String {
    let values: BTreeMap<&str, &String> = values.iter().map(|(k, v)| (*k, v)).collect();
    serde_json::json!({ "say": template, "values": values }).to_string()
}
