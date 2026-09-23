//! Scoring and a live runner for the labeled triage eval in `evals/triage`.
//! `evals/README.md` explains how to run it and how to read the report.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::triage;
use crate::typesafe::{self, Answers, Client};

const CASES: &str = "evals/triage/cases.jsonl";
const WORKSPACES: &str = "evals/triage/workspaces.json";
const RESULTS_DIR: &str = "target/triage-eval";
const RESULTS_ENV: &str = "TRIAGE_EVAL_RESULTS";
/// Generous, so slow answers are measured instead of cut off; the report
/// counts how many would have missed `triage::BUDGET`.
const REQUEST_BUDGET: Duration = Duration::from_secs(30);
const TOOL_THRESHOLDS: [f64; 8] = [0.02, 0.05, 0.1, 0.15, 0.2, 0.3, 0.4, 0.5];
const EFFORT_CUTOFFS: [f64; 8] = [0.0, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9];
const LEVELS: [&str; 3] = ["recall", "explain", "investigate"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Label {
    Needed,
    Unneeded,
    Either,
}

#[derive(Clone, Debug)]
struct Case {
    id: String,
    workspace: String,
    question: String,
    conversation: Vec<(String, String)>,
    needs_workspace: Label,
    needs_web: Label,
    /// Acceptable effort levels as indexes into `LEVELS`. The first one is
    /// the label the confusion matrix uses.
    effort: Vec<usize>,
}

struct Workspace {
    folder: String,
    entries: Vec<String>,
}

/// One request's outcome, as saved under `target/triage-eval`.
#[derive(Clone, Debug, PartialEq)]
struct Run {
    id: String,
    millis: u64,
    response: Option<Value>,
    error: Option<String>,
}

/// What Jev said about one case, read through the same accessors the
/// product uses.
#[derive(Clone, Debug, Default, PartialEq)]
struct Observation {
    workspace: Option<f64>,
    web: Option<f64>,
    effort: Option<(usize, f64)>,
}

struct ToolRow {
    threshold: f64,
    false_drops: Vec<(String, f64)>,
    dropped: usize,
}

struct ToolScore {
    needed: usize,
    unneeded: usize,
    either: usize,
    unanswered: usize,
    rows: Vec<ToolRow>,
}

struct EffortRow {
    cutoff: f64,
    applied: usize,
    correct: usize,
    too_little: Vec<(String, f64)>,
}

struct EffortScore {
    confusion: [[usize; 3]; 3],
    answered: usize,
    unanswered: usize,
    rows: Vec<EffortRow>,
}

struct Latency {
    p50: u64,
    p95: u64,
    max: u64,
    over_budget: usize,
}

#[test]
#[ignore = "calls the TypeSafe API; see evals/README.md"]
fn triage_eval() {
    let cases = load_cases();
    let (runs, questions) = match std::env::var_os(RESULTS_ENV) {
        Some(path) => load_runs(Path::new(&path)),
        None => {
            let Some(client) = Client::from_env() else {
                println!(
                    "skipping the triage eval: set {} to run it against Jev",
                    typesafe::ENV_KEY
                );
                return;
            };
            let runs = run_live(&client, &cases);
            println!("saved Jev's answers to {}", save_runs(&runs).display());
            (runs, triage::questions())
        }
    };
    if questions != triage::questions() {
        println!("note: these answers were recorded with different triage questions\n");
    }
    println!("{}", report(&cases, &runs));
    assert!(
        runs.iter().any(|run| run.response.is_some()),
        "TypeSafe answered none of the cases"
    );
}

fn run_live(client: &Client, cases: &[Case]) -> Vec<Run> {
    let base = std::env::temp_dir().join(format!("wut-triage-eval-{}", std::process::id()));
    let roots = materialize(&load_workspaces(), &base);
    let mut runs = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let state = triage::state(&case.question, &case.conversation, &roots[&case.workspace]);
        let started = Instant::now();
        let result = client.system_one(state, triage::questions(), REQUEST_BUDGET);
        let millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        eprintln!("[{}/{}] {} ({millis} ms)", index + 1, cases.len(), case.id);
        let (response, error, permanent) = match result {
            Ok(answers) => (Some(answers.response().clone()), None, false),
            Err(failure) => (
                None,
                Some(failure.error.message().to_owned()),
                failure.permanent,
            ),
        };
        runs.push(Run {
            id: case.id.clone(),
            millis,
            response,
            error,
        });
        if permanent {
            break;
        }
    }
    let _ = fs::remove_dir_all(&base);
    runs
}

fn manifest_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn load_cases() -> Vec<Case> {
    let path = manifest_path(CASES);
    let text =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str::<Value>(line)
                .map_err(|error| error.to_string())
                .and_then(|value| parse_case(&value))
                .unwrap_or_else(|problem| panic!("{CASES} line {}: {problem}", index + 1))
        })
        .collect()
}

fn parse_case(value: &Value) -> Result<Case, String> {
    let text = |key: &str| {
        value[key]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("'{key}' must be a non-empty string"))
    };
    let label = |key: &str| match &value[key] {
        Value::Bool(true) => Ok(Label::Needed),
        Value::Bool(false) => Ok(Label::Unneeded),
        Value::String(text) if text == "either" => Ok(Label::Either),
        other => Err(format!(
            "'{key}' must be true, false, or \"either\", not {other}"
        )),
    };
    let levels = match &value["effort"] {
        Value::Array(levels) => levels.iter().collect(),
        level => vec![level],
    };
    let effort = levels
        .into_iter()
        .map(|level| {
            level
                .as_str()
                .and_then(level_index)
                .ok_or_else(|| format!("unknown effort level {level}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if effort.is_empty() {
        return Err("'effort' needs at least one level".to_owned());
    }
    let mut conversation = Vec::new();
    for turn in value["conversation"].as_array().into_iter().flatten() {
        match (turn["user"].as_str(), turn["assistant"].as_str()) {
            (Some(user), Some(assistant)) => {
                conversation.push((user.to_owned(), assistant.to_owned()));
            }
            _ => return Err("conversation turns need 'user' and 'assistant'".to_owned()),
        }
    }
    text("why")?;
    Ok(Case {
        id: text("id")?,
        workspace: text("workspace")?,
        question: text("question")?,
        conversation,
        needs_workspace: label("needs_workspace")?,
        needs_web: label("needs_web")?,
        effort,
    })
}

fn level_index(name: &str) -> Option<usize> {
    LEVELS.iter().position(|level| *level == name)
}

fn load_workspaces() -> BTreeMap<String, Workspace> {
    let path = manifest_path(WORKSPACES);
    let text =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let value: Value =
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{WORKSPACES}: {error}"));
    let workspaces = value
        .as_object()
        .unwrap_or_else(|| panic!("{WORKSPACES} must hold an object"));
    workspaces
        .iter()
        .map(|(key, workspace)| {
            let folder = workspace["folder"]
                .as_str()
                .unwrap_or_else(|| panic!("workspace '{key}' needs a folder"));
            let entries = workspace["entries"]
                .as_array()
                .unwrap_or_else(|| panic!("workspace '{key}' needs entries"))
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_owned())
                .collect();
            let workspace = Workspace {
                folder: folder.to_owned(),
                entries,
            };
            (key.clone(), workspace)
        })
        .collect()
}

/// Lays each workspace out under `base` as empty files and directories, so
/// the eval sends exactly the listing `triage::state` builds for it.
fn materialize(workspaces: &BTreeMap<String, Workspace>, base: &Path) -> HashMap<String, PathBuf> {
    workspaces
        .iter()
        .map(|(key, workspace)| {
            let root = base.join(key).join(&workspace.folder);
            fs::create_dir_all(&root).unwrap();
            for entry in &workspace.entries {
                match entry.strip_suffix('/') {
                    Some(directory) => fs::create_dir_all(root.join(directory)).unwrap(),
                    None => fs::write(root.join(entry), "").unwrap(),
                }
            }
            (key.clone(), root)
        })
        .collect()
}

impl Run {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "millis": self.millis,
            "response": self.response,
            "error": self.error,
        })
    }

    fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            id: value["id"].as_str()?.to_owned(),
            millis: value["millis"].as_u64()?,
            response: Some(value["response"].clone()).filter(|response| !response.is_null()),
            error: value["error"].as_str().map(str::to_owned),
        })
    }
}

fn save_runs(runs: &[Run]) -> PathBuf {
    let directory = manifest_path(RESULTS_DIR);
    fs::create_dir_all(&directory).unwrap();
    let recorded = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let path = directory.join(format!("results-{recorded}.json"));
    let saved = json!({
        "recorded_at": recorded,
        "questions": triage::questions(),
        "runs": runs.iter().map(Run::to_json).collect::<Vec<_>>(),
    });
    fs::write(&path, serde_json::to_string_pretty(&saved).unwrap()).unwrap();
    path
}

fn load_runs(path: &Path) -> (Vec<Run>, Value) {
    let text =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let saved: Value =
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let runs = saved["runs"]
        .as_array()
        .unwrap_or_else(|| panic!("{} has no 'runs' array", path.display()))
        .iter()
        .map(|run| Run::from_json(run).expect("each saved run has an id and millis"))
        .collect();
    (runs, saved["questions"].clone())
}

impl Observation {
    fn read(response: &Value) -> Self {
        let answers = Answers::from_response(response.clone());
        Self {
            workspace: answers.noul(triage::NEEDS_WORKSPACE),
            web: answers.noul(triage::NEEDS_WEB),
            effort: answers
                .choice(triage::THINKING)
                .and_then(|choice| Some((level_index(&choice.choice)?, choice.confidence))),
        }
    }
}

/// `values` plus `current`, sorted, so the product's setting is always a row.
fn sweep(values: &[f64], current: f64) -> Vec<f64> {
    let mut values = values.to_vec();
    values.push(current);
    values.sort_by(f64::total_cmp);
    values.dedup_by(|a, b| same(*a, *b));
    values
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

impl ToolScore {
    /// Mirrors `Plan::from_answers`: a tool is dropped when its probability
    /// is at or below the threshold, and kept when Jev gave no answer.
    fn new(
        scored: &[(&Case, Observation)],
        label: fn(&Case) -> Label,
        probability: fn(&Observation) -> Option<f64>,
    ) -> Self {
        let count = |wanted: Label| {
            scored
                .iter()
                .filter(|(case, _)| label(case) == wanted)
                .count()
        };
        let rows = sweep(&TOOL_THRESHOLDS, triage::TOOL_UNNEEDED_MAX)
            .into_iter()
            .map(|threshold| {
                let mut row = ToolRow {
                    threshold,
                    false_drops: Vec::new(),
                    dropped: 0,
                };
                for (case, observation) in scored {
                    let Some(p) = probability(observation).filter(|p| *p <= threshold) else {
                        continue;
                    };
                    match label(case) {
                        Label::Needed => row.false_drops.push((case.id.clone(), p)),
                        Label::Unneeded => row.dropped += 1,
                        Label::Either => {}
                    }
                }
                row
            })
            .collect();
        Self {
            needed: count(Label::Needed),
            unneeded: count(Label::Unneeded),
            either: count(Label::Either),
            unanswered: scored
                .iter()
                .filter(|(_, observation)| probability(observation).is_none())
                .count(),
            rows,
        }
    }

    /// The highest swept threshold that drops no needed tool.
    fn highest_safe_threshold(&self) -> Option<f64> {
        self.rows
            .iter()
            .rev()
            .find(|row| row.false_drops.is_empty())
            .map(|row| row.threshold)
    }
}

impl EffortScore {
    fn new(scored: &[(&Case, Observation)]) -> Self {
        let mut confusion = [[0; 3]; 3];
        let mut answered = 0;
        for (case, observation) in scored {
            if let Some((level, _)) = observation.effort {
                confusion[case.effort[0]][level] += 1;
                answered += 1;
            }
        }
        let rows = sweep(&EFFORT_CUTOFFS, triage::MIN_EFFORT_CONFIDENCE)
            .into_iter()
            .map(|cutoff| {
                let mut row = EffortRow {
                    cutoff,
                    applied: 0,
                    correct: 0,
                    too_little: Vec::new(),
                };
                for (case, observation) in scored {
                    let Some((level, confidence)) = observation.effort else {
                        continue;
                    };
                    if confidence < cutoff {
                        continue;
                    }
                    row.applied += 1;
                    if case.effort.contains(&level) {
                        row.correct += 1;
                    } else if case.effort.iter().all(|acceptable| level < *acceptable) {
                        row.too_little.push((case.id.clone(), confidence));
                    }
                }
                row
            })
            .collect();
        Self {
            confusion,
            answered,
            unanswered: scored.len() - answered,
            rows,
        }
    }

    /// The lowest swept cutoff at which Jev never picks less effort than a
    /// question needs. Picking too much only costs time; too little costs
    /// the answer.
    fn lowest_safe_cutoff(&self) -> Option<f64> {
        self.rows
            .iter()
            .find(|row| row.too_little.is_empty())
            .map(|row| row.cutoff)
    }
}

fn latency(millis: &[u64]) -> Option<Latency> {
    let mut sorted = millis.to_vec();
    sorted.sort_unstable();
    let max = *sorted.last()?;
    let rank = |quantile: f64| sorted[((sorted.len() - 1) as f64 * quantile).round() as usize];
    let budget = u64::try_from(triage::BUDGET.as_millis()).unwrap_or(u64::MAX);
    Some(Latency {
        p50: rank(0.5),
        p95: rank(0.95),
        max,
        over_budget: sorted.iter().filter(|millis| **millis > budget).count(),
    })
}

fn share(part: usize, whole: usize) -> String {
    if whole == 0 {
        return "-".to_owned();
    }
    format!("{part} of {whole} ({}%)", (part * 100 + whole / 2) / whole)
}

fn report(cases: &[Case], runs: &[Run]) -> String {
    let by_id: HashMap<&str, &Run> = runs.iter().map(|run| (run.id.as_str(), run)).collect();
    let mut scored = Vec::new();
    let mut failed = Vec::new();
    let mut millis = Vec::new();
    let mut models = BTreeSet::new();
    let mut tokens = Vec::new();
    for case in cases {
        let Some(run) = by_id.get(case.id.as_str()) else {
            continue;
        };
        match &run.response {
            Some(response) => {
                scored.push((case, Observation::read(response)));
                millis.push(run.millis);
                models.extend(response["model"].as_str().map(str::to_owned));
                tokens.extend(response["usage"]["input_tokens"].as_u64());
            }
            None => failed.push((&case.id, run.error.as_deref().unwrap_or("no answer"))),
        }
    }

    let mut out = String::new();
    let models = if models.is_empty() {
        "an unreported model".to_owned()
    } else {
        models.into_iter().collect::<Vec<_>>().join(", ")
    };
    let _ = writeln!(
        out,
        "Triage eval: {} cases, {} answered by {models}",
        cases.len(),
        scored.len()
    );
    let missing = cases.len() - scored.len() - failed.len();
    if missing > 0 {
        let _ = writeln!(out, "{missing} cases are not in these results");
    }
    for (id, error) in &failed {
        let _ = writeln!(out, "failed: {id}: {error}");
    }
    if let Some(latency) = latency(&millis) {
        let _ = writeln!(
            out,
            "Latency: p50 {} ms, p95 {} ms, max {} ms; {} over the {:?} triage budget",
            latency.p50,
            latency.p95,
            latency.max,
            latency.over_budget,
            triage::BUDGET
        );
    }
    if !tokens.is_empty() {
        let mean = tokens.iter().sum::<u64>() / tokens.len() as u64;
        let _ = writeln!(out, "Input tokens: {mean} per request on average");
    }
    let workspace = ToolScore::new(&scored, |case| case.needs_workspace, |seen| seen.workspace);
    tool_section(&mut out, triage::NEEDS_WORKSPACE, &workspace);
    let web = ToolScore::new(&scored, |case| case.needs_web, |seen| seen.web);
    tool_section(&mut out, triage::NEEDS_WEB, &web);
    effort_section(&mut out, &EffortScore::new(&scored), &scored);
    out
}

fn tool_section(out: &mut String, name: &str, score: &ToolScore) {
    let _ = writeln!(
        out,
        "\n{name}: {} needed, {} not needed, {} either",
        score.needed, score.unneeded, score.either
    );
    if score.unanswered > 0 {
        let _ = writeln!(
            out,
            "  {} cases got no answer, so the tool stays on",
            score.unanswered
        );
    }
    let _ = writeln!(
        out,
        "    drop when p <=   needed but dropped   not needed and dropped"
    );
    for row in &score.rows {
        let current = if same(row.threshold, triage::TOOL_UNNEEDED_MAX) {
            '*'
        } else {
            ' '
        };
        let _ = writeln!(
            out,
            "  {current} {:>14.2}   {:>18}   {:>22}",
            row.threshold,
            row.false_drops.len(),
            share(row.dropped, score.unneeded)
        );
    }
    let _ = writeln!(out, "  * current threshold");
    match score.highest_safe_threshold() {
        Some(threshold) => {
            let _ = writeln!(
                out,
                "  highest threshold with no false drops: {threshold:.2}"
            );
        }
        None => {
            let _ = writeln!(out, "  every swept threshold drops a needed tool");
        }
    }
    let current = score
        .rows
        .iter()
        .find(|row| same(row.threshold, triage::TOOL_UNNEEDED_MAX));
    for (id, p) in current.into_iter().flat_map(|row| &row.false_drops) {
        let _ = writeln!(
            out,
            "  dropped at the current threshold: {id} (needed, p={p:.2})"
        );
    }
}

fn effort_section(out: &mut String, score: &EffortScore, scored: &[(&Case, Observation)]) {
    let _ = writeln!(
        out,
        "\n{}: rows are the labeled effort, columns are Jev's choice",
        triage::THINKING
    );
    let _ = writeln!(
        out,
        "  {:>12} {:>8} {:>8} {:>12}",
        "", LEVELS[0], LEVELS[1], LEVELS[2]
    );
    for (label, counts) in LEVELS.iter().zip(score.confusion) {
        let _ = writeln!(
            out,
            "  {label:>12} {:>8} {:>8} {:>12}",
            counts[0], counts[1], counts[2]
        );
    }
    if score.unanswered > 0 {
        let _ = writeln!(
            out,
            "  {} cases got no answer, so they use the configured effort",
            score.unanswered
        );
    }
    let _ = writeln!(
        out,
        "    confidence >=           applied           correct   too little"
    );
    for row in &score.rows {
        let current = if same(row.cutoff, triage::MIN_EFFORT_CONFIDENCE) {
            '*'
        } else {
            ' '
        };
        let _ = writeln!(
            out,
            "  {current} {:>15.2}   {:>15}   {:>15}   {:>10}",
            row.cutoff,
            share(row.applied, score.answered),
            share(row.correct, row.applied),
            row.too_little.len()
        );
    }
    let _ = writeln!(out, "  * current cutoff");
    match score.lowest_safe_cutoff() {
        Some(cutoff) => {
            let _ = writeln!(
                out,
                "  lowest cutoff where Jev never picks too little effort: {cutoff:.2}"
            );
        }
        None => {
            let _ = writeln!(out, "  Jev picks too little effort at every swept cutoff");
        }
    }
    for (case, observation) in scored {
        let Some((level, confidence)) = observation.effort else {
            continue;
        };
        if confidence < triage::MIN_EFFORT_CONFIDENCE || case.effort.contains(&level) {
            continue;
        }
        let labeled = case
            .effort
            .iter()
            .map(|level| LEVELS[*level])
            .collect::<Vec<_>>()
            .join(" or ");
        let _ = writeln!(
            out,
            "  disagrees at the current cutoff: {}: labeled {labeled}, Jev chose {} ({confidence:.2})",
            case.id, LEVELS[level]
        );
    }
}

mod tests {
    use std::collections::HashSet;
    use std::fs;

    use serde_json::{Value, json};

    use super::{
        Case, EffortScore, Label, Observation, Run, ToolScore, latency, load_cases,
        load_workspaces, materialize, report, same,
    };
    use crate::triage;

    const RECALL: usize = 0;
    const EXPLAIN: usize = 1;
    const INVESTIGATE: usize = 2;

    fn case(id: &str, workspace: Label, web: Label, effort: &[usize]) -> Case {
        Case {
            id: id.to_owned(),
            workspace: "scratch".to_owned(),
            question: format!("question {id}"),
            conversation: Vec::new(),
            needs_workspace: workspace,
            needs_web: web,
            effort: effort.to_vec(),
        }
    }

    fn seen(workspace: f64, effort: Option<(usize, f64)>) -> Observation {
        Observation {
            workspace: Some(workspace),
            web: Some(0.0),
            effort,
        }
    }

    fn normalize(text: &str) -> String {
        text.to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn examples(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    match (key.as_str(), value) {
                        ("examples", Value::Array(list)) => {
                            found.extend(list.iter().filter_map(Value::as_str).map(normalize));
                        }
                        _ => examples(value, found),
                    }
                }
            }
            Value::Array(values) => values.iter().for_each(|value| examples(value, found)),
            _ => {}
        }
    }

    #[test]
    fn eval_data_is_well_formed() {
        let cases = load_cases();
        let workspaces = load_workspaces();
        assert!(cases.len() >= 50, "the eval needs at least 50 cases");
        let mut ids = HashSet::new();
        for case in &cases {
            assert!(
                ids.insert(case.id.as_str()),
                "duplicate case id {}",
                case.id
            );
            assert!(
                workspaces.contains_key(&case.workspace),
                "{} uses an unknown workspace {}",
                case.id,
                case.workspace
            );
        }
    }

    #[test]
    fn eval_cases_do_not_reuse_prompt_examples() {
        let mut found = Vec::new();
        examples(&triage::questions(), &mut found);
        assert!(found.len() > 10);
        for case in load_cases() {
            assert!(
                !found.contains(&normalize(&case.question)),
                "{} repeats an example from the triage questions",
                case.id
            );
        }
    }

    #[test]
    fn materialized_workspaces_match_what_triage_sends() {
        let base =
            std::env::temp_dir().join(format!("wut-triage-eval-test-{}", std::process::id()));
        let roots = materialize(&load_workspaces(), &base);
        let state = triage::state("q", &[], &roots["webapp"]);
        let _ = fs::remove_dir_all(&base);
        assert_eq!(state["workspace"]["folder"], "storefront");
        let entries: Vec<&str> = state["workspace"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(entries.contains(&"package.json"));
        assert!(entries.contains(&"src/"));
        assert!(
            !entries.contains(&".env"),
            "secret files never reach TypeSafe"
        );
    }

    #[test]
    fn scores_tool_thresholds_like_the_plan() {
        let cases = [
            case("needed-low", Label::Needed, Label::Unneeded, &[RECALL]),
            case("needed-high", Label::Needed, Label::Unneeded, &[RECALL]),
            case("unneeded-low", Label::Unneeded, Label::Unneeded, &[RECALL]),
            case("unneeded-edge", Label::Unneeded, Label::Unneeded, &[RECALL]),
            case("either", Label::Either, Label::Unneeded, &[RECALL]),
        ];
        let observations = [
            seen(0.12, None),
            seen(0.9, None),
            seen(0.01, None),
            seen(0.2, None),
            seen(0.0, None),
        ];
        let scored: Vec<_> = cases.iter().zip(observations).collect();
        let score = ToolScore::new(&scored, |case| case.needs_workspace, |seen| seen.workspace);
        assert_eq!((score.needed, score.unneeded, score.either), (2, 2, 1));
        let row = |threshold: f64| {
            score
                .rows
                .iter()
                .find(|row| same(row.threshold, threshold))
                .unwrap()
        };
        assert!(row(0.1).false_drops.is_empty());
        assert_eq!(row(0.1).dropped, 1);
        assert_eq!(row(0.15).false_drops, [("needed-low".to_owned(), 0.12)]);
        assert_eq!(row(0.2).dropped, 2, "p at the threshold is dropped");
        assert_eq!(score.highest_safe_threshold(), Some(0.1));
    }

    #[test]
    fn scores_effort_by_confidence() {
        let cases = [
            case("recall", Label::Either, Label::Either, &[RECALL]),
            case("investigate", Label::Either, Label::Either, &[INVESTIGATE]),
            case(
                "either",
                Label::Either,
                Label::Either,
                &[EXPLAIN, INVESTIGATE],
            ),
            case("silent", Label::Either, Label::Either, &[EXPLAIN]),
        ];
        let observations = [
            seen(0.5, Some((EXPLAIN, 0.9))),
            seen(0.5, Some((RECALL, 0.45))),
            seen(0.5, Some((INVESTIGATE, 0.6))),
            seen(0.5, None),
        ];
        let scored: Vec<_> = cases.iter().zip(observations).collect();
        let score = EffortScore::new(&scored);
        assert_eq!((score.answered, score.unanswered), (3, 1));
        assert_eq!(score.confusion[RECALL][EXPLAIN], 1);
        assert_eq!(score.confusion[INVESTIGATE][RECALL], 1);
        assert_eq!(score.confusion[EXPLAIN][INVESTIGATE], 1);
        let everything = &score.rows[0];
        assert_eq!((everything.applied, everything.correct), (3, 1));
        assert_eq!(everything.too_little, [("investigate".to_owned(), 0.45)]);
        assert_eq!(score.lowest_safe_cutoff(), Some(0.5));
    }

    #[test]
    fn summarizes_latency_against_the_budget() {
        let summary = latency(&[900, 100, 300, 200, 2500]).unwrap();
        assert_eq!(
            (summary.p50, summary.p95, summary.max, summary.over_budget),
            (300, 2500, 2500, 1)
        );
        assert!(latency(&[]).is_none());
    }

    #[test]
    fn saved_runs_round_trip() {
        let answered = Run {
            id: "a".to_owned(),
            millis: 12,
            response: Some(json!({"answers": {}})),
            error: None,
        };
        assert_eq!(Run::from_json(&answered.to_json()), Some(answered));
        let failed = Run {
            id: "b".to_owned(),
            millis: 3,
            response: None,
            error: Some("boom".to_owned()),
        };
        assert_eq!(Run::from_json(&failed.to_json()), Some(failed));
    }

    #[test]
    fn report_reads_answers_like_the_product() {
        let cases = [
            case("repo", Label::Needed, Label::Unneeded, &[INVESTIGATE]),
            case("flaky", Label::Unneeded, Label::Unneeded, &[RECALL]),
        ];
        let runs = [
            Run {
                id: "repo".to_owned(),
                millis: 150,
                response: Some(json!({
                    "model": "jev-1.13.0",
                    "answers": {
                        "needs_workspace_files": {"type": "noul", "noul": 0.05},
                        "needs_current_web_information": {"type": "noul", "noul": 0.01},
                        "thinking_required": {
                            "type": "choice",
                            "choice": "recall",
                            "probabilities": {"recall": 0.8, "explain": 0.15, "investigate": 0.05},
                            "confidence": 0.7
                        }
                    },
                    "usage": {"input_tokens": 900, "output_tokens": 30}
                })),
                error: None,
            },
            Run {
                id: "flaky".to_owned(),
                millis: 2100,
                response: None,
                error: Some("TypeSafe returned HTTP 529".to_owned()),
            },
        ];
        let text = report(&cases, &runs);
        for expected in [
            "Triage eval: 2 cases, 1 answered by jev-1.13.0",
            "failed: flaky: TypeSafe returned HTTP 529",
            "p50 150 ms",
            "0 over the 2s triage budget",
            "Input tokens: 900 per request on average",
            "needs_workspace_files: 1 needed, 0 not needed, 0 either",
            "dropped at the current threshold: repo (needed, p=0.05)",
            "needs_current_web_information: 0 needed, 1 not needed, 0 either",
            "disagrees at the current cutoff: repo: labeled investigate, Jev chose recall (0.70)",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
    }
}
