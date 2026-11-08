use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::cerebras;
use crate::error::{Error, Result};
use crate::instructions::Instructions;
use crate::storage;

pub struct Config {
    model: Option<String>,
    reasoning: Option<String>,
    response_color: ResponseColor,
    instructions: Instructions,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResponseColor {
    #[default]
    Default,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    Orange,
    Pink,
    Purple,
    Teal,
    Lime,
    Sky,
    CatppuccinRosewater,
    CatppuccinFlamingo,
    CatppuccinPink,
    CatppuccinMauve,
    CatppuccinRed,
    CatppuccinMaroon,
    CatppuccinPeach,
    CatppuccinYellow,
    CatppuccinGreen,
    CatppuccinTeal,
    CatppuccinSky,
    CatppuccinSapphire,
    CatppuccinBlue,
    CatppuccinLavender,
}

impl ResponseColor {
    pub const ALL: [Self; 34] = [
        Self::Default,
        Self::Red,
        Self::Green,
        Self::Yellow,
        Self::Blue,
        Self::Magenta,
        Self::Cyan,
        Self::White,
        Self::BrightRed,
        Self::BrightGreen,
        Self::BrightYellow,
        Self::BrightBlue,
        Self::BrightMagenta,
        Self::BrightCyan,
        Self::Orange,
        Self::Pink,
        Self::Purple,
        Self::Teal,
        Self::Lime,
        Self::Sky,
        Self::CatppuccinRosewater,
        Self::CatppuccinFlamingo,
        Self::CatppuccinPink,
        Self::CatppuccinMauve,
        Self::CatppuccinRed,
        Self::CatppuccinMaroon,
        Self::CatppuccinPeach,
        Self::CatppuccinYellow,
        Self::CatppuccinGreen,
        Self::CatppuccinTeal,
        Self::CatppuccinSky,
        Self::CatppuccinSapphire,
        Self::CatppuccinBlue,
        Self::CatppuccinLavender,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "Terminal default",
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Yellow => "Yellow",
            Self::Blue => "Blue",
            Self::Magenta => "Magenta",
            Self::Cyan => "Cyan",
            Self::White => "White",
            Self::BrightRed => "Bright red",
            Self::BrightGreen => "Bright green",
            Self::BrightYellow => "Bright yellow",
            Self::BrightBlue => "Bright blue",
            Self::BrightMagenta => "Bright magenta",
            Self::BrightCyan => "Bright cyan",
            Self::Orange => "Orange",
            Self::Pink => "Pink",
            Self::Purple => "Purple",
            Self::Teal => "Teal",
            Self::Lime => "Lime",
            Self::Sky => "Sky blue",
            Self::CatppuccinRosewater => "Catppuccin Rosewater",
            Self::CatppuccinFlamingo => "Catppuccin Flamingo",
            Self::CatppuccinPink => "Catppuccin Pink",
            Self::CatppuccinMauve => "Catppuccin Mauve",
            Self::CatppuccinRed => "Catppuccin Red",
            Self::CatppuccinMaroon => "Catppuccin Maroon",
            Self::CatppuccinPeach => "Catppuccin Peach",
            Self::CatppuccinYellow => "Catppuccin Yellow",
            Self::CatppuccinGreen => "Catppuccin Green",
            Self::CatppuccinTeal => "Catppuccin Teal",
            Self::CatppuccinSky => "Catppuccin Sky",
            Self::CatppuccinSapphire => "Catppuccin Sapphire",
            Self::CatppuccinBlue => "Catppuccin Blue",
            Self::CatppuccinLavender => "Catppuccin Lavender",
        }
    }

    pub fn ansi(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Red => "\x1b[97;41m",
            Self::Green => "\x1b[97;42m",
            Self::Yellow => "\x1b[30;43m",
            Self::Blue => "\x1b[97;44m",
            Self::Magenta => "\x1b[97;45m",
            Self::Cyan => "\x1b[30;46m",
            Self::White => "\x1b[30;47m",
            Self::BrightRed => "\x1b[30;101m",
            Self::BrightGreen => "\x1b[30;102m",
            Self::BrightYellow => "\x1b[30;103m",
            Self::BrightBlue => "\x1b[30;104m",
            Self::BrightMagenta => "\x1b[30;105m",
            Self::BrightCyan => "\x1b[30;106m",
            Self::Orange => "\x1b[30;48;5;208m",
            Self::Pink => "\x1b[30;48;5;213m",
            Self::Purple => "\x1b[30;48;5;141m",
            Self::Teal => "\x1b[30;48;5;43m",
            Self::Lime => "\x1b[30;48;5;118m",
            Self::Sky => "\x1b[30;48;5;117m",
            Self::CatppuccinRosewater => "\x1b[30;48;2;245;224;220m",
            Self::CatppuccinFlamingo => "\x1b[30;48;2;242;205;205m",
            Self::CatppuccinPink => "\x1b[30;48;2;245;194;231m",
            Self::CatppuccinMauve => "\x1b[30;48;2;203;166;247m",
            Self::CatppuccinRed => "\x1b[30;48;2;243;139;168m",
            Self::CatppuccinMaroon => "\x1b[30;48;2;235;160;172m",
            Self::CatppuccinPeach => "\x1b[30;48;2;250;179;135m",
            Self::CatppuccinYellow => "\x1b[30;48;2;249;226;175m",
            Self::CatppuccinGreen => "\x1b[30;48;2;166;227;161m",
            Self::CatppuccinTeal => "\x1b[30;48;2;148;226;213m",
            Self::CatppuccinSky => "\x1b[30;48;2;137;220;235m",
            Self::CatppuccinSapphire => "\x1b[30;48;2;116;199;236m",
            Self::CatppuccinBlue => "\x1b[30;48;2;137;180;250m",
            Self::CatppuccinLavender => "\x1b[30;48;2;180;190;254m",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Red => "red",
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Blue => "blue",
            Self::Magenta => "magenta",
            Self::Cyan => "cyan",
            Self::White => "white",
            Self::BrightRed => "bright-red",
            Self::BrightGreen => "bright-green",
            Self::BrightYellow => "bright-yellow",
            Self::BrightBlue => "bright-blue",
            Self::BrightMagenta => "bright-magenta",
            Self::BrightCyan => "bright-cyan",
            Self::Orange => "orange",
            Self::Pink => "pink",
            Self::Purple => "purple",
            Self::Teal => "teal",
            Self::Lime => "lime",
            Self::Sky => "sky",
            Self::CatppuccinRosewater => "catppuccin-rosewater",
            Self::CatppuccinFlamingo => "catppuccin-flamingo",
            Self::CatppuccinPink => "catppuccin-pink",
            Self::CatppuccinMauve => "catppuccin-mauve",
            Self::CatppuccinRed => "catppuccin-red",
            Self::CatppuccinMaroon => "catppuccin-maroon",
            Self::CatppuccinPeach => "catppuccin-peach",
            Self::CatppuccinYellow => "catppuccin-yellow",
            Self::CatppuccinGreen => "catppuccin-green",
            Self::CatppuccinTeal => "catppuccin-teal",
            Self::CatppuccinSky => "catppuccin-sky",
            Self::CatppuccinSapphire => "catppuccin-sapphire",
            Self::CatppuccinBlue => "catppuccin-blue",
            Self::CatppuccinLavender => "catppuccin-lavender",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|color| color.id() == value)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: Some(cerebras::default_model().to_owned()),
            reasoning: cerebras::find_model(cerebras::default_model())
                .and_then(|model| model.default_reasoning.map(str::to_owned)),
            response_color: ResponseColor::default(),
            instructions: Instructions::default(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = path()?;
        if path.exists() {
            load_file(&path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        storage::write_private(&path()?, &self.bytes()?, "wut config")
    }

    fn bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec_pretty(&json!({
            "version": 4,
            "model": self.model,
            "reasoning": self.reasoning,
            "response_color": self.response_color.id(),
            "instructions": self.instructions.to_json(),
        }))
        .map_err(|error| Error::internal(format!("could not encode wut config: {error}")))
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub fn set_model(&mut self, model: Option<String>) {
        self.model = model;
    }

    pub fn reasoning(&self) -> Option<&str> {
        self.reasoning.as_deref()
    }

    pub fn set_reasoning(&mut self, reasoning: Option<String>) {
        self.reasoning = reasoning;
    }

    pub fn response_color(&self) -> ResponseColor {
        self.response_color
    }

    pub fn set_response_color(&mut self, color: ResponseColor) {
        self.response_color = color;
    }

    pub fn instructions(&self) -> &Instructions {
        &self.instructions
    }

    pub fn set_instructions(&mut self, instructions: Instructions) {
        self.instructions = instructions;
    }

    fn from_value(value: &Value) -> Result<Self> {
        let version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
        if version != 4 {
            return Err(Error::new(
                format!("config version {version} is not supported"),
                "remove the config file, then run 'wut --settings'",
            ));
        }
        let response_color = value
            .get("response_color")
            .and_then(Value::as_str)
            .and_then(ResponseColor::parse)
            .ok_or_else(|| invalid_config("config has invalid 'response_color'"))?;
        let (model, reasoning) = cerebras::migrate_archived_model(
            optional_string(value, "model")?,
            optional_string(value, "reasoning")?,
        );
        Ok(Self {
            model: normalize_model(model)?,
            reasoning,
            response_color,
            instructions: Instructions::from_json(value.get("instructions"))?,
        })
    }
}

fn normalize_model(model: Option<String>) -> Result<Option<String>> {
    let Some(model) = model else {
        return Ok(Some(cerebras::default_model().to_owned()));
    };
    Ok(Some(cerebras::require_model(&model)?.id.to_owned()))
}

fn load_file(path: &Path) -> Result<Config> {
    let bytes = fs::read(path).map_err(|error| {
        Error::new(
            format!("could not read '{}': {error}", path.display()),
            "check its permissions and try again",
        )
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        Error::new(
            format!("could not parse '{}': {error}", path.display()),
            "fix or remove the file, then try again",
        )
    })?;
    Config::from_value(&value)
        .map_err(|error| error.context(format!("could not load '{}'", path.display())))
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid_config(format!("config has invalid '{key}'"))),
    }
}

fn path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("WUT_CONFIG").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path).join("wut/config.json"));
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            Error::new(
                "HOME is not set",
                "set XDG_CONFIG_HOME to a writable directory and try again",
            )
        })?;
    Ok(PathBuf::from(home).join(".config/wut/config.json"))
}

fn invalid_config(message: impl Into<String>) -> Error {
    Error::new(message, "fix or remove the config file, then try again")
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Config, ResponseColor};
    use crate::cerebras;
    use crate::instructions::Instructions;

    fn load(value: Value) -> crate::error::Result<Config> {
        Config::from_value(&value)
    }

    #[test]
    fn saved_config_loads_back_unchanged() {
        let mut config = Config::default();
        config.set_model(Some(cerebras::default_model().to_owned()));
        config.set_reasoning(Some("high".to_owned()));
        config.set_response_color(ResponseColor::CatppuccinMauve);
        config.set_instructions(Instructions::Custom("be terse".to_owned()));
        let saved: Value = serde_json::from_slice(&config.bytes().unwrap()).unwrap();
        assert_eq!(saved["version"], 4);

        let loaded = load(saved).unwrap();
        assert_eq!(loaded.model(), Some(cerebras::default_model()));
        assert_eq!(loaded.reasoning(), Some("high"));
        assert_eq!(loaded.response_color(), ResponseColor::CatppuccinMauve);
        assert_eq!(
            loaded.instructions(),
            &Instructions::Custom("be terse".to_owned())
        );
    }

    #[test]
    fn every_response_color_round_trips_by_id() {
        for color in ResponseColor::ALL {
            assert_eq!(ResponseColor::parse(color.id()), Some(color));
        }
        assert_eq!(ResponseColor::parse("chartreuse"), None);
    }

    #[test]
    fn rejects_any_other_version() {
        for version in [json!(0), json!(3), json!(5), json!(null)] {
            let error = load(json!({"version": version, "response_color": "default"}))
                .err()
                .unwrap();
            assert!(error.message().contains("is not supported"), "{version}");
        }
    }

    #[test]
    fn rejects_invalid_fields() {
        let color = load(json!({"version": 4, "response_color": "chartreuse"}))
            .err()
            .unwrap();
        assert!(color.message().contains("response_color"));
        let missing_color = load(json!({"version": 4})).err().unwrap();
        assert!(missing_color.message().contains("response_color"));
        let model = load(json!({"version": 4, "response_color": "default", "model": 7}))
            .err()
            .unwrap();
        assert!(model.message().contains("'model'"));
        let instructions =
            load(json!({"version": 4, "response_color": "default", "instructions": "loud"}))
                .err()
                .unwrap();
        assert!(instructions.message().contains("instructions"));
    }

    #[test]
    fn a_missing_model_uses_the_default() {
        let config = load(json!({"version": 4, "response_color": "default"})).unwrap();
        assert_eq!(config.model(), Some(cerebras::default_model()));
        assert_eq!(config.reasoning(), None);
        assert_eq!(config.instructions(), &Instructions::Concise);
    }

    #[test]
    fn groq_models_survive_config_round_trips_and_unknown_models_fail() {
        let mut config = Config::default();
        config.set_model(Some("groq:openai/gpt-oss-20b".to_owned()));
        config.set_reasoning(Some("low".to_owned()));
        let saved = serde_json::from_slice(&config.bytes().unwrap()).unwrap();
        let restored = load(saved).unwrap();
        assert_eq!(restored.model(), Some("groq:openai/gpt-oss-20b"));
        assert_eq!(restored.reasoning(), Some("low"));
        let error = load(json!({"version": 4, "response_color": "default", "model": "typo"}))
            .err()
            .unwrap();
        assert!(error.message().contains("unknown model"));
    }

    #[test]
    fn archived_gemma_defaults_migrate_to_an_available_cerebras_model() {
        for (old_effort, new_effort) in
            [(Some("none"), "low"), (None, "low"), (Some("high"), "high")]
        {
            let config = load(json!({"version": 4, "response_color": "default", "model": "gemma-4-31b", "reasoning": old_effort})).unwrap();
            assert_eq!(config.model(), Some("gpt-oss-120b"));
            assert_eq!(config.reasoning(), Some(new_effort));
        }
    }
}
