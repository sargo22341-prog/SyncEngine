//! Presentation models decoded from the facade's bounded JSON views, plus
//! French labels. No comparison or synchronization decision is made here:
//! every value displayed comes from the engines.

use serde::Deserialize;
use serde_json::Value;
use sync_plan::{Action, DeletionRefusal, SyncMode};
use sync_profiles::{CompareMethod, Profile, RemoteEndpoint};

/// Counts computed by the planner for one preview.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct Summary {
    pub(crate) copy: u64,
    pub(crate) replace: u64,
    pub(crate) delete: u64,
    pub(crate) keep: u64,
    pub(crate) conflict: u64,
    pub(crate) skip: u64,
    pub(crate) bytes: u64,
    pub(crate) deletions_refused: u64,
    /// Guard that refused every deletion of the plan (all or nothing).
    #[serde(default)]
    pub(crate) deletion_refusal: Option<DeletionRefusal>,
    /// The user lifted the deletion ceiling for this preview only.
    #[serde(default)]
    pub(crate) ceiling_lifted: bool,
}

impl Summary {
    pub(crate) fn mutations(&self) -> u64 {
        self.copy + self.replace + self.delete
    }
    pub(crate) fn total(&self) -> u64 {
        self.copy + self.replace + self.delete + self.keep + self.conflict + self.skip
    }
    pub(crate) fn count(&self, action: Action) -> u64 {
        match action {
            Action::Copy => self.copy,
            Action::Replace => self.replace,
            Action::Delete => self.delete,
            Action::Keep => self.keep,
            Action::Conflict => self.conflict,
            Action::Skip => self.skip,
            _ => 0,
        }
    }
}

/// One side of an operation as observed during the scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) kind: String,
    pub(crate) size: Option<u64>,
    pub(crate) modified: Option<i64>,
}

/// Displayable operation of the exact cached plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpView {
    pub(crate) path: String,
    pub(crate) action: Action,
    pub(crate) reason: String,
    pub(crate) bytes: Option<u64>,
    pub(crate) to_source: bool,
    pub(crate) kind: String,
    pub(crate) sender: Option<Observation>,
    pub(crate) receiver: Option<Observation>,
}

fn observation(value: &Value) -> Option<Observation> {
    if value.is_null() {
        return None;
    }
    Some(Observation {
        kind: value["kind"].as_str().unwrap_or("").to_owned(),
        size: value["metadata"]["size"].as_u64(),
        modified: value["metadata"]["modified"]["secs"].as_i64(),
    })
}

impl OpView {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        let action: Action = serde_json::from_value(value.get("action")?.clone()).ok()?;
        Some(Self {
            path: value["path"].as_str()?.to_owned(),
            action,
            reason: value["reason"]["code"].as_str().unwrap_or("").to_owned(),
            bytes: value["bytes"].as_u64(),
            to_source: value["target"].as_str() == Some("source"),
            kind: value["kind"].as_str().unwrap_or("").to_owned(),
            sender: observation(&value["source"]),
            receiver: observation(&value["destination"]),
        })
    }
}

/// Paged preview returned by `preview_paged`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreviewView {
    pub(crate) plan_id: String,
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) destination: String,
    pub(crate) source_complete: bool,
    pub(crate) destination_complete: bool,
    pub(crate) summary: Summary,
    pub(crate) first: Vec<OpView>,
}

impl PreviewView {
    pub(crate) fn parse(value: &Value) -> Result<Self, String> {
        let plan = &value["plan"];
        let summary: Summary = serde_json::from_value(value["summary"].clone())
            .map_err(|e| format!("résumé de simulation illisible : {e}"))?;
        Ok(Self {
            plan_id: plan["id"]
                .as_str()
                .ok_or("identifiant de plan absent")?
                .to_owned(),
            profile: plan["profile"].as_str().unwrap_or("").to_owned(),
            source: pretty_path(plan["source"]["location"].as_str().unwrap_or("")),
            destination: pretty_path(plan["destination"]["location"].as_str().unwrap_or("")),
            source_complete: plan["source"]["complete"].as_bool().unwrap_or(false),
            destination_complete: plan["destination"]["complete"].as_bool().unwrap_or(false),
            summary,
            first: ops(&plan["operations"]),
        })
    }
}

pub(crate) fn ops(value: &Value) -> Vec<OpView> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(OpView::parse).collect())
        .unwrap_or_default()
}

/// Outcome of one historical operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryRow {
    pub(crate) op: Option<OpView>,
    pub(crate) status: String,
    pub(crate) detail: String,
    pub(crate) version: Option<String>,
}

pub(crate) fn history_rows(value: &Value) -> Vec<HistoryRow> {
    value["operations"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let status = &row["outcome"]["status"];
                    let detail = status["error"]["message"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| status["refusal"]["cause"].as_str().map(str::to_owned))
                        .or_else(|| {
                            status["bytes"]
                                .as_u64()
                                .map(|b| format!("{} vérifiés", bytes(b)))
                        })
                        .unwrap_or_default();
                    HistoryRow {
                        op: OpView::parse(&row["operation"]),
                        status: status["status"].as_str().unwrap_or("").to_owned(),
                        detail,
                        version: status["version"].as_str().map(str::to_owned),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Classification of a persisted attempt, used for colors and icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Success,
    Warning,
    Failure,
    Interrupted,
}

/// Bounded summary of one attempt (`history_overview`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RunView {
    pub(crate) execution: String,
    pub(crate) started: Option<i64>,
    pub(crate) outcome: Outcome,
    pub(crate) label: String,
    pub(crate) message: Option<String>,
    pub(crate) totals: Vec<(String, u64)>,
}

impl RunView {
    pub(crate) fn from_run(run: &sync_orchestrator::Run) -> Self {
        let started = serde_json::from_str::<Value>(&run.started)
            .ok()
            .and_then(|v| v["secs"].as_i64());
        let report = run.report.clone().unwrap_or(Value::Null);
        let (outcome, label) = classify(&run.state, &report);
        let message = report["message"]
            .as_str()
            .or_else(|| report["verification_error"].as_str())
            .map(str::to_owned)
            .or_else(|| {
                report["code"]
                    .as_str()
                    .filter(|c| *c != "interruption_acknowledged")
                    .map(str::to_owned)
            });
        let totals = report["totals"]
            .as_object()
            .map(|map| {
                [
                    "done",
                    "no_action",
                    "refused",
                    "failed",
                    "cancelled",
                    "not_started",
                    "bytes",
                ]
                .iter()
                .filter_map(|key| {
                    map.get(*key)
                        .and_then(Value::as_u64)
                        .map(|v| ((*key).to_owned(), v))
                })
                .collect()
            })
            .unwrap_or_default();
        Self {
            execution: run.execution.clone(),
            started,
            outcome,
            label,
            message,
            totals,
        }
    }
}

/// Maps the durable state/report to a visible status. Unknown values stay
/// « à inspecter » rather than being presented as successes.
pub(crate) fn classify(state: &str, report: &Value) -> (Outcome, String) {
    match state {
        "running" => {
            return (
                Outcome::Interrupted,
                "Interrompu ou en cours — diagnostic requis".into(),
            );
        }
        "error" => {
            if report["code"].as_str() == Some("interruption_acknowledged") {
                return (Outcome::Failure, "Interruption diagnostiquée".into());
            }
            return (Outcome::Failure, "Échec — consulter le diagnostic".into());
        }
        _ => {}
    }
    if report["verification_error"]
        .as_str()
        .is_some_and(|s| !s.is_empty())
    {
        return (
            Outcome::Failure,
            "Échec de vérification de l’état de référence".into(),
        );
    }
    let status = report["status"]
        .as_str()
        .or_else(|| report["execution"]["status"].as_str())
        .unwrap_or("");
    let refused = report["deletions_refused"].as_u64().unwrap_or(0);
    match status {
        "succeeded" if refused > 0 => (
            Outcome::Warning,
            format!("Miroir incomplet — {refused} suppression(s) refusée(s)"),
        ),
        "succeeded" => (Outcome::Success, "Réussi".into()),
        "succeeded_with_warnings" => (Outcome::Warning, "Terminé avec avertissements".into()),
        "cancelled" => (Outcome::Warning, "Annulé — état partiel conservé".into()),
        "failed" => (Outcome::Failure, "Échec — état partiel conservé".into()),
        "interrupted" => (
            Outcome::Interrupted,
            "Interrompu — diagnostic requis".into(),
        ),
        _ => (Outcome::Warning, "Résultat à inspecter".into()),
    }
}

pub(crate) fn mode_label(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Backup => "Sauvegarde",
        SyncMode::Mirror => "Miroir",
        SyncMode::Bidirectional => "Bidirectionnel",
        _ => "Mode inconnu",
    }
}

pub(crate) fn compare_label(compare: CompareMethod) -> &'static str {
    match compare {
        CompareMethod::SizeAndDate => "Taille et date",
        CompareMethod::Content => "Contenu (BLAKE3)",
    }
}

pub(crate) fn mode_tagline(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Backup => "Copie vers la destination, ne supprime jamais rien.",
        SyncMode::Mirror => "Rend la destination identique à la source, sous protections.",
        SyncMode::Bidirectional => "Propage les changements des deux côtés, conflits conservés.",
        _ => "",
    }
}

pub(crate) fn action_label(action: Action) -> &'static str {
    match action {
        Action::Copy => "Copier",
        Action::Replace => "Remplacer",
        Action::Delete => "Supprimer",
        Action::Keep => "Conserver",
        Action::Conflict => "Conflit",
        Action::Skip => "Ignorer",
        _ => "Autre",
    }
}

pub(crate) fn action_key(action: Action) -> &'static str {
    match action {
        Action::Copy => "copy",
        Action::Replace => "replace",
        Action::Delete => "delete",
        Action::Keep => "keep",
        Action::Conflict => "conflict",
        Action::Skip => "skip",
        _ => "",
    }
}

pub(crate) fn reason_label(code: &str) -> &str {
    match code {
        "missing_at_destination" => "Absent de la destination",
        "source_newer" => "Source plus récente",
        "destination_newer" => "Destination plus récente",
        "content_differs" => "Contenu différent",
        "ambiguous_change" => "Modification ambiguë",
        "kind_mismatch" => "Types différents : conflit à résoudre",
        "equivalent" => "Éléments identiques",
        "destination_only" => "Présent uniquement à destination",
        "overwrite_disabled" => "Remplacement interdit par le profil",
        "deletion_disabled" => "Suppression interdite par le profil",
        "absent_from_source" => "Absent de la source : suppression prévue",
        "uncertain" => "Observation incertaine : aucune mutation",
        "unsupported_entry_kind" => "Lien ou type non pris en charge",
        "parent_unavailable" => "Dossier parent indisponible",
        "directory_not_emptied" => "Dossier conservé : contenu non supprimable",
        "deletion_refused" => "Suppression bloquée par un garde-fou",
        "concurrent_change" => "Modifications concurrentes : conflit",
        other => other,
    }
}

/// Exact cause of a refused set of deletions, as decided by the planner.
pub(crate) fn refusal_label(refusal: DeletionRefusal) -> String {
    match refusal {
        DeletionRefusal::SourceIncomplete => "La source n’a pas pu être lue entièrement : une absence n’y prouve rien, aucune suppression n’est appliquée.".into(),
        DeletionRefusal::DestinationIncomplete => "La destination n’a pas pu être lue entièrement : aucune suppression n’est appliquée.".into(),
        DeletionRefusal::SourceEmpty => "La source est vide (disque débranché, mauvais dossier ?) : aucune suppression n’est appliquée à la destination.".into(),
        DeletionRefusal::TooMany { count: n, limit } => format!(
            "{} suppressions dépassent le plafond de {} : rien n’est supprimé tant que vous ne l’autorisez pas.",
            count(n),
            count(limit)
        ),
        DeletionRefusal::TooLarge { count: n, total, max_percent } => format!(
            "{} suppressions sur {} éléments de la destination dépassent le plafond de {max_percent} % (protection contre un mauvais dossier) : rien n’est supprimé tant que vous ne l’autorisez pas.",
            count(n),
            count(total)
        ),
        _ => "Un garde-fou de suppression s’est déclenché : aucune suppression n’est appliquée.".into(),
    }
}

/// Only volume ceilings can be lifted by the user; an empty source or an
/// incomplete inventory never can.
pub(crate) fn ceiling_liftable(refusal: DeletionRefusal) -> bool {
    matches!(
        refusal,
        DeletionRefusal::TooMany { .. } | DeletionRefusal::TooLarge { .. }
    )
}

pub(crate) fn outcome_label(status: &str) -> &str {
    match status {
        "done" => "Réalisée",
        "no_action" => "Aucune mutation",
        "refused" => "Refusée",
        "failed" => "Échec",
        "cancelled" => "Annulée",
        "not_started" => "Non commencée",
        "" => "Résultat non enregistré",
        other => other,
    }
}

pub(crate) fn total_label(key: &str) -> &str {
    match key {
        "done" => "réalisées",
        "no_action" => "sans mutation",
        "refused" => "refusées",
        "failed" => "en échec",
        "cancelled" => "annulées",
        "not_started" => "non commencées",
        "bytes" => "vérifiés",
        other => other,
    }
}

/// Human readable provider name of an endpoint.
pub(crate) fn provider_label(remote: Option<&RemoteEndpoint>) -> &'static str {
    match remote {
        None => "Dossier local",
        Some(RemoteEndpoint::Smb { .. }) => "Partage SMB",
        Some(RemoteEndpoint::Sftp { .. }) => "SFTP",
        Some(RemoteEndpoint::Webdav { .. }) => "WebDAV",
        Some(RemoteEndpoint::Ftp { .. }) => "FTP non chiffré",
        Some(RemoteEndpoint::Ftps { .. }) => "FTPS",
        Some(RemoteEndpoint::Onedrive { .. }) => "OneDrive (expérimental)",
    }
}

/// Short location of an endpoint, without credentials (none are stored).
pub(crate) fn endpoint_location(
    local: &std::path::Path,
    remote: Option<&RemoteEndpoint>,
) -> String {
    match remote {
        None => local.display().to_string(),
        Some(RemoteEndpoint::Smb { root }) => root.display().to_string(),
        Some(RemoteEndpoint::Sftp {
            host,
            username,
            root,
            ..
        }) => format!("{username}@{host}:{root}"),
        Some(RemoteEndpoint::Webdav { root, .. }) => root.clone(),
        Some(RemoteEndpoint::Ftp {
            address,
            username,
            root,
            ..
        })
        | Some(RemoteEndpoint::Ftps {
            address,
            username,
            root,
            ..
        }) => format!("{username}@{address}{root}"),
        Some(RemoteEndpoint::Onedrive { drive_id, .. }) => format!("lecteur {drive_id}"),
    }
}

pub(crate) fn source_of(profile: &Profile) -> String {
    endpoint_location(&profile.source, profile.remote_source.as_ref())
}

pub(crate) fn destination_of(profile: &Profile) -> String {
    endpoint_location(&profile.destination, profile.remote_destination.as_ref())
}

/// Binary-prefixed byte count with French decimal comma.
pub(crate) fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} o")
    } else {
        format!("{size:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

/// `done / total` clamped to `0.0..=1.0`; `None` when the total is unknown (0).
pub(crate) fn fraction(done: u64, total: u64) -> Option<f32> {
    (total > 0).then(|| (done as f64 / total as f64).min(1.0) as f32)
}

/// Progress percentage: one decimal below 10 % so a long run visibly moves
/// (« 1,4 % »), whole numbers above; never rounds up to 100 % before the end.
pub(crate) fn percent(fraction: f32) -> String {
    // Truncated, not rounded; the tiny margin only absorbs the f32
    // representation error (0.42 is stored as 0.41999…).
    const EPSILON: f64 = 1e-4;
    let value = f64::from(fraction.clamp(0.0, 1.0)) * 100.0;
    if value < 10.0 {
        format!("{:.1} %", (value * 10.0 + EPSILON).floor() / 10.0).replace('.', ",")
    } else {
        format!("{:.0} %", (value + EPSILON).floor())
    }
}

/// Integer with thin-space thousands separators (French typography).
pub(crate) fn count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

/// Civil UTC date from Unix seconds (Howard Hinnant's algorithm).
pub(crate) fn utc(secs: i64) -> (i64, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year,
        month,
        day,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
    )
}

pub(crate) fn utc_label(secs: i64) -> String {
    let (y, m, d, h, min) = utc(secs);
    format!("{d:02}/{m:02}/{y} {h:02}:{min:02} UTC")
}

/// « il y a 5 min », « dans 2 h » relative to `now` (Unix seconds).
pub(crate) fn relative(secs: i64, now: i64) -> String {
    let delta = now - secs;
    let past = delta >= 0;
    let d = delta.unsigned_abs();
    let span = match d {
        0..=44 => {
            return if past {
                "à l’instant".into()
            } else {
                "dans un instant".into()
            };
        }
        45..=3_599 => format!("{} min", d.div_ceil(60).max(1)),
        3_600..=86_399 => format!("{} h", d / 3600),
        86_400..=2_591_999 => format!("{} j", d / 86_400),
        _ => return utc_label(secs),
    };
    if past {
        format!("il y a {span}")
    } else {
        format!("dans {span}")
    }
}

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Removes the Windows verbatim prefix added by canonicalization.
pub(crate) fn pretty_path(value: &str) -> String {
    value
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| value.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| value.to_owned())
}

/// Shortens a long path keeping both ends visible.
pub(crate) fn ellipsize(value: &str, max: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max || max < 5 {
        return value.to_owned();
    }
    let keep = max - 1;
    let head = keep / 3;
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_or_partial_reports_are_never_shown_as_success() {
        assert_eq!(classify("running", &Value::Null).0, Outcome::Interrupted);
        assert_eq!(
            classify("error", &json!({"code":"busy"})).0,
            Outcome::Failure
        );
        assert_eq!(classify("finished", &json!({})).0, Outcome::Warning);
        assert_eq!(
            classify("finished", &json!({"status":"cancelled"})).0,
            Outcome::Warning
        );
        assert_eq!(
            classify("finished", &json!({"execution":{"status":"succeeded"}})).0,
            Outcome::Success
        );
        let (outcome, label) = classify(
            "finished",
            &json!({"status":"succeeded","deletions_refused":2}),
        );
        assert_eq!(outcome, Outcome::Warning);
        assert!(label.contains('2'));
        assert_eq!(
            classify(
                "finished",
                &json!({"status":"succeeded","verification_error":"x"})
            )
            .0,
            Outcome::Failure
        );
    }

    #[test]
    fn preview_and_operations_are_decoded_from_the_facade_shape() {
        let value = json!({
            "plan": {"id":"plan-1","profile":"p","mode":"mirror",
                "source":{"connector":"local","location":"C:/a","complete":true,"entries":3},
                "destination":{"connector":"local","location":"C:/b","complete":false,"entries":1},
                "operations":[{"id":"op-1","path":"docs/a.txt","kind":"file","action":"delete",
                    "reason":{"code":"absent_from_source"},"risk":"destructive","preconditions":[],
                    "depends_on":[],"bytes":null,"source":null,
                    "destination":{"kind":"file","metadata":{"size":12,"modified":{"secs":10,"nanos":0}}}}]},
            "summary":{"copy":1,"replace":0,"delete":1,"keep":4,"conflict":0,"skip":0,"bytes":9,"deletions_refused":0}
        });
        let preview = PreviewView::parse(&value).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(preview.plan_id, "plan-1");
        assert!(preview.source_complete && !preview.destination_complete);
        assert_eq!(preview.summary.total(), 6);
        assert_eq!(preview.summary.mutations(), 2);
        let op = &preview.first[0];
        assert_eq!(op.action, Action::Delete);
        assert_eq!(op.receiver.as_ref().and_then(|o| o.size), Some(12));
        assert!(op.sender.is_none());
        assert!(PreviewView::parse(&json!({"plan":{},"summary":{}})).is_err());
        assert_eq!(preview.summary.deletion_refusal, None);
        assert!(!preview.summary.ceiling_lifted);
    }

    #[test]
    fn deletion_refusal_is_decoded_and_only_ceilings_are_liftable() {
        let summary: Summary = serde_json::from_value(json!({
            "copy":0,"replace":0,"delete":0,"keep":1,"conflict":0,"skip":12,"bytes":0,
            "deletions_refused":12,
            "deletion_refusal":{"cause":"too_large","count":12,"total":13,"max_percent":50},
            "ceiling_lifted":false
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        let refusal = summary
            .deletion_refusal
            .unwrap_or_else(|| panic!("refusal"));
        assert!(ceiling_liftable(refusal));
        assert!(refusal_label(refusal).contains("50 %"));
        assert!(ceiling_liftable(DeletionRefusal::TooMany {
            count: 3,
            limit: 2
        }));
        for blocked in [
            DeletionRefusal::SourceEmpty,
            DeletionRefusal::SourceIncomplete,
            DeletionRefusal::DestinationIncomplete,
        ] {
            assert!(!ceiling_liftable(blocked));
        }
    }

    #[test]
    fn progress_is_weighted_and_formatted_honestly() {
        // Real run: 9 906 thumbnails (831 MB) of 41 098 copies totalling
        // 58.4 GB. Volume dominates: the run is at 1.4 %, not 24 %.
        let done = 831_288_005u64 + 9_906;
        let total = 58_424_037_921u64 + 41_099;
        let f = fraction(done, total).unwrap_or_default();
        assert_eq!(percent(f), "1,4 %");
        // A plan without bytes (folders, deletions) still moves.
        assert_eq!(fraction(3, 4), Some(0.75));
        assert_eq!(fraction(5, 0), None);
        assert_eq!(fraction(9, 4), Some(1.0));
        assert_eq!(percent(0.0), "0,0 %");
        assert_eq!(percent(0.0999), "9,9 %");
        assert_eq!(percent(0.42), "42 %");
        // Never shows 100 % before the end.
        assert_eq!(percent(0.9999), "99 %");
        assert_eq!(percent(1.0), "100 %");
    }

    #[test]
    fn formatting_helpers() {
        assert_eq!(bytes(512), "512 o");
        assert_eq!(bytes(1536), "1,5 Ko");
        assert_eq!(count(1_234_567), "1\u{202f}234\u{202f}567");
        assert_eq!(count(12), "12");
        assert_eq!(utc(0), (1970, 1, 1, 0, 0));
        assert_eq!(utc(1_792_756_800), (2026, 10, 23, 12, 0));
        assert_eq!(relative(100, 130), "à l’instant");
        assert_eq!(relative(0, 7_200), "il y a 2 h");
        assert_eq!(relative(7_200, 0), "dans 2 h");
        assert_eq!(ellipsize("abcdefghij", 6), "a…ghij");
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(pretty_path(r"\\?\C:\data"), r"C:\data");
        assert_eq!(pretty_path(r"\\?\UNC\nas\share"), r"\\nas\share");
        assert_eq!(pretty_path("/home/a"), "/home/a");
    }
}
