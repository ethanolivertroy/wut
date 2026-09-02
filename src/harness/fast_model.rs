use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::{Model, Response};
use crate::error::Result;
use crate::storage;

pub(super) const CACHE_TTL_SECONDS: u64 = 24 * 60 * 60;

/// The model id every harness accepts as "whatever is fastest right now".
pub(super) const ALIAS: &str = "fast";

/// Streams one turn against a concrete model id, forwarding answer deltas.
pub(super) type RunOnce<'a> =
    dyn FnMut(&str, &mut dyn FnMut(&str) -> Result<()>) -> Result<Response> + 'a;

/// Models served from Cerebras wafer-scale hardware (1000+ tokens/s), most
/// capable coding model first. Deprecated ids stay so that agents still
/// configured with them keep resolving; nothing here is ever the *only* way
/// a Cerebras model is recognised (see `cerebras_id`).
const CEREBRAS_MODELS: &[&str] = &[
    "gpt-oss-120b",
    "gemma-4-31b",
    "zai-glm-4.7",
    "qwen-3-coder-480b",
    "zai-glm-4.6",
    "qwen-3-235b-a22b-instruct-2507",
    "qwen-3-32b",
    "llama-4-scout-17b-16e-instruct",
    "llama-3.3-70b",
    "llama3.1-8b",
];

/// Identify a Cerebras-hosted model and return its bare Cerebras id.
///
/// Agents surface Cerebras three ways: as a `cerebras/` provider prefix
/// (OpenCode, Pi), as bare Cerebras ids when the whole CLI is pointed at
/// `api.cerebras.ai` (Grok Build with `GROK_MODELS_BASE_URL`), or as a
/// custom endpoint the user labelled "cerebras" themselves.
fn cerebras_id(model: &Model) -> Option<&str> {
    if let Some(id) = model.id.strip_prefix("cerebras/") {
        return Some(id);
    }
    if CEREBRAS_MODELS.contains(&model.id.as_str()) {
        return Some(&model.id);
    }
    [&model.id, &model.name, &model.description]
        .iter()
        .any(|text| text.to_ascii_lowercase().contains("cerebras"))
        .then_some(model.id.as_str())
}

/// The best Cerebras-hosted model in a catalog, if any.
pub(super) fn cerebras_model(models: &[Model]) -> Option<&Model> {
    let hosted = models
        .iter()
        .filter_map(|model| cerebras_id(model).map(|id| (model, id)))
        .collect::<Vec<_>>();
    CEREBRAS_MODELS
        .iter()
        .find_map(|preferred| {
            hosted
                .iter()
                .find(|(_, id)| id == preferred)
                .map(|(model, _)| *model)
        })
        .or_else(|| hosted.first().map(|(model, _)| *model))
}

/// Prepend a `fast` catalog entry that names the model it currently maps to.
pub(super) fn with_alias(mut models: Vec<Model>, target: Option<&Model>) -> Vec<Model> {
    if models.iter().any(|model| model.id == ALIAS) {
        return models;
    }
    if let Some(target) = target {
        models.insert(
            0,
            Model {
                id: ALIAS.into(),
                name: "Fastest available".into(),
                description: format!("Currently uses {}", target.name),
                is_default: false,
                reasoning: target.reasoning.clone(),
                default_reasoning: target.default_reasoning.clone(),
            },
        );
    }
    models
}

/// Resolution state for the `fast` alias within one harness instance.
pub(super) struct Alias {
    cache: Cache,
    resolved: Option<String>,
    from_disk: bool,
}

impl Alias {
    pub(super) const fn new(agent: &'static str) -> Self {
        Self {
            cache: Cache::new(agent),
            resolved: None,
            from_disk: false,
        }
    }

    /// The concrete model behind `fast`: memory, then disk, then `refresh`.
    fn model(&mut self, refresh: &dyn Fn() -> Result<String>) -> Result<String> {
        if let Some(model) = &self.resolved {
            return Ok(model.clone());
        }
        if let Some(model) = self.cache.read() {
            self.from_disk = true;
            self.resolved = Some(model.clone());
            return Ok(model);
        }
        self.refresh(refresh)
    }

    fn refresh(&mut self, refresh: &dyn Fn() -> Result<String>) -> Result<String> {
        let model = refresh()?;
        self.cache.write(&model);
        self.from_disk = false;
        self.resolved = Some(model.clone());
        Ok(model)
    }

    /// Run one turn against the resolved fast model.
    ///
    /// A model remembered on disk may have been retired since it was
    /// resolved. When such a turn fails before anything was shown, the cache
    /// is dropped, the alias re-resolved once, and the turn retried, but only
    /// if the catalog now names a different model; otherwise the failure has
    /// some other cause and a second run would just repeat it.
    pub(super) fn run(
        &mut self,
        refresh: &dyn Fn() -> Result<String>,
        run_once: &mut RunOnce<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Response> {
        let model = self.model(refresh)?;
        let mut streamed = false;
        let result = run_once(&model, &mut |delta| {
            streamed = true;
            on_delta(delta)
        });
        let Err(error) = result else {
            return result;
        };
        if !self.from_disk || streamed {
            return Err(error);
        }
        self.cache.invalidate();
        self.resolved = None;
        self.from_disk = false;
        let fresh = self.refresh(refresh)?;
        if fresh == model {
            return Err(error);
        }
        run_once(&fresh, on_delta)
    }
}

/// Per-agent disk memo of the concrete model the `fast` alias resolved to.
///
/// Resolving the alias needs a catalog round-trip to the provider, which is
/// pure overhead on the hot path of a one-shot question. The memo is
/// best-effort: it saves a round-trip, never fails a turn, and callers
/// invalidate it when the provider rejects the remembered model.
pub(super) struct Cache {
    agent: &'static str,
}

impl Cache {
    pub(super) const fn new(agent: &'static str) -> Self {
        Self { agent }
    }

    pub(super) fn read(&self) -> Option<String> {
        read_cached_model(&self.path()?, now())
    }

    pub(super) fn write(&self, model: &str) {
        if let Some(path) = self.path() {
            write_cached_model(&path, model, now(), self.agent);
        }
    }

    pub(super) fn invalidate(&self) {
        if let Some(path) = self.path() {
            invalidate_cached_model(&path);
        }
    }

    fn path(&self) -> Option<PathBuf> {
        cache_path_from(
            self.agent,
            std::env::var_os("XDG_CACHE_HOME"),
            std::env::var_os("HOME"),
        )
    }
}

fn cache_path_from(
    agent: &str,
    xdg_cache_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    let file = format!("wut/{agent}.json");
    if let Some(path) = xdg_cache_home.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path).join(file));
    }
    home.filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".cache").join(file))
}

fn read_cached_model(path: &Path, now: u64) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let model = value.get("fast_model")?.as_str()?;
    if model.is_empty() {
        return None;
    }
    let resolved_at = value.get("resolved_at")?.as_u64()?;
    // A resolution timestamp in the future means the clock moved backwards;
    // treat the entry as stale rather than trusting it indefinitely.
    let age = now.checked_sub(resolved_at)?;
    (age < CACHE_TTL_SECONDS).then(|| model.to_owned())
}

fn write_cached_model(path: &Path, model: &str, resolved_at: u64, agent: &str) {
    let value = json!({
        "fast_model": model,
        "resolved_at": resolved_at,
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&value) {
        let _ = storage::write_private(path, &bytes, &format!("{agent} model cache"));
    }
}

fn invalidate_cached_model(path: &Path) {
    let _ = fs::remove_file(path);
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        CACHE_TTL_SECONDS, cache_path_from, cerebras_model, invalidate_cached_model,
        read_cached_model, with_alias, write_cached_model,
    };
    use crate::harness::Model;

    fn model(id: &str) -> Model {
        Model {
            id: id.into(),
            name: id.rsplit('/').next().unwrap_or(id).into(),
            description: String::new(),
            is_default: false,
            reasoning: Vec::new(),
            default_reasoning: None,
        }
    }

    #[test]
    fn cerebras_models_are_recognised_by_prefix_bare_id_or_label() {
        let prefixed = [
            model("anthropic/claude-sonnet"),
            model("cerebras/gemma-4-31b"),
            model("cerebras/gpt-oss-120b"),
        ];
        assert_eq!(
            cerebras_model(&prefixed).unwrap().id,
            "cerebras/gpt-oss-120b"
        );

        let bare = [
            model("grok-4.6"),
            model("gemma-4-31b"),
            model("gpt-oss-120b"),
        ];
        assert_eq!(cerebras_model(&bare).unwrap().id, "gpt-oss-120b");

        let mut labelled = model("company-fast");
        labelled.description = "GPT OSS via Cerebras proxy".into();
        let labelled = [model("grok-4.6"), labelled];
        assert_eq!(cerebras_model(&labelled).unwrap().id, "company-fast");

        let unknown_cerebras = [model("cerebras/brand-new-model")];
        assert_eq!(
            cerebras_model(&unknown_cerebras).unwrap().id,
            "cerebras/brand-new-model"
        );

        assert!(cerebras_model(&[model("openai/gpt-5.4"), model("groq/llama")]).is_none());
        assert!(cerebras_model(&[]).is_none());
    }

    #[test]
    fn alias_is_prepended_once_and_names_its_target() {
        let target = model("cerebras/gpt-oss-120b");
        let models = with_alias(vec![model("grok-4.6"), target.clone()], Some(&target));
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "fast");
        assert_eq!(models[0].name, "Fastest available");
        assert_eq!(models[0].description, "Currently uses gpt-oss-120b");
        assert!(!models[0].is_default);

        let again = with_alias(models.clone(), Some(&target));
        assert_eq!(again.len(), 3);

        let none = with_alias(vec![model("grok-4.6")], None);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].id, "grok-4.6");
    }

    fn unique_cache_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("wut-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn cache_round_trips_until_the_ttl_expires() {
        let directory = unique_cache_directory("fast-cache-ttl");
        let path = directory.join("codex.json");

        write_cached_model(&path, "gpt-5.3-codex-spark", 1_000, "codex");

        assert_eq!(
            read_cached_model(&path, 1_000).as_deref(),
            Some("gpt-5.3-codex-spark")
        );
        assert_eq!(
            read_cached_model(&path, 1_000 + CACHE_TTL_SECONDS - 1).as_deref(),
            Some("gpt-5.3-codex-spark")
        );
        assert_eq!(read_cached_model(&path, 1_000 + CACHE_TTL_SECONDS), None);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_rejects_clock_rollback_and_malformed_entries() {
        let directory = unique_cache_directory("fast-cache-invalid");
        let path = directory.join("grok.json");

        assert_eq!(read_cached_model(&path, 1_000), None);

        write_cached_model(&path, "grok-code-fast-1", 2_000, "grok");
        assert_eq!(read_cached_model(&path, 1_999), None);

        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, b"not-json").unwrap();
        assert_eq!(read_cached_model(&path, 1_000), None);

        fs::write(&path, b"{\"fast_model\":\"\",\"resolved_at\":1000}").unwrap();
        assert_eq!(read_cached_model(&path, 1_000), None);

        fs::write(&path, b"{\"fast_model\":\"grok\"}").unwrap();
        assert_eq!(read_cached_model(&path, 1_000), None);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalidating_the_cache_removes_the_entry() {
        let directory = unique_cache_directory("fast-cache-invalidate");
        let path = directory.join("codex.json");

        write_cached_model(&path, "gpt-5.3-codex-spark", 1_000, "codex");
        assert!(read_cached_model(&path, 1_000).is_some());

        invalidate_cached_model(&path);
        assert_eq!(read_cached_model(&path, 1_000), None);

        invalidate_cached_model(&path);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_paths_are_per_agent_and_never_relative() {
        assert_eq!(cache_path_from("codex", None, Some(OsString::new())), None);
        assert_eq!(cache_path_from("codex", None, None), None);
        assert_eq!(
            cache_path_from("codex", Some(OsString::from("/cache")), None),
            Some(PathBuf::from("/cache/wut/codex.json"))
        );
        assert_eq!(
            cache_path_from("grok", None, Some(OsString::from("/home/user"))),
            Some(PathBuf::from("/home/user/.cache/wut/grok.json"))
        );
    }
}
