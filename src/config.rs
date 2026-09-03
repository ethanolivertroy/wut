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
        let (canonical, legacy) = paths()?;
        if canonical.exists() {
            return load_file(&canonical);
        }
        let Some(legacy) = legacy.filter(|legacy| legacy.exists()) else {
            return Ok(Self::default());
        };
        import_legacy(&canonical, &legacy)
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
        let version = value.get("version").and_then(Value::as_u64).unwrap_or(1);
        if version > 4 {
            return Err(Error::new(
                format!("config version {version} requires a newer version of wut"),
                "run 'wut --upgrade'",
            ));
        }
        if version == 0 {
            return Err(Error::new(
                "config version 0 is not supported",
                "remove the config file, then run 'wut --settings'",
            ));
        }

        let instructions_version = version.min(2);
        let mut config = Self {
            instructions: Instructions::from_json(value.get("instructions"), instructions_version)?,
            ..Self::default()
        };
        if version >= 3 {
            config.model = normalize_model(optional_string(value, "model")?);
            config.reasoning = optional_string(value, "reasoning")?;
            if version >= 4 {
                let color = value
                    .get("response_color")
                    .and_then(Value::as_str)
                    .and_then(ResponseColor::parse)
                    .ok_or_else(|| invalid_config("config has invalid 'response_color'"))?;
                config.response_color = color;
            }
            return Ok(config);
        }
        migrate_v2(value, &mut config);
        Ok(config)
    }
}

fn migrate_v2(value: &Value, config: &mut Config) {
    let agent = value["agent"].as_str().unwrap_or_default();
    let settings = value["agents"]
        .as_object()
        .and_then(|agents| agents.get(agent).or_else(|| agents.get("cerebras")));
    let raw = settings
        .and_then(|settings| settings["model"].as_str())
        .or_else(|| value["model"].as_str())
        .unwrap_or_default();
    let id = raw.strip_prefix("cerebras/").unwrap_or(raw);
    let Some(model) = cerebras::find_model(id) else {
        return;
    };
    config.model = Some(model.id.to_owned());
    config.reasoning = settings
        .and_then(|settings| settings["reasoning"].as_str())
        .or_else(|| value["reasoning"].as_str())
        .map(str::to_owned)
        .or_else(|| model.default_reasoning.map(str::to_owned));
}

fn normalize_model(model: Option<String>) -> Option<String> {
    let Some(model) = model else {
        return Some(cerebras::default_model().to_owned());
    };
    let id = model.strip_prefix("cerebras/").unwrap_or(&model);
    Some(cerebras::resolve_model(Some(id)).to_owned())
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

fn import_legacy(canonical: &Path, legacy: &Path) -> Result<Config> {
    let legacy_config = load_file(legacy)?;
    if storage::write_private_if_absent(canonical, &legacy_config.bytes()?, "wut config migration")?
    {
        Ok(legacy_config)
    } else {
        load_file(canonical)
    }
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid_config(format!("config has invalid '{key}'"))),
    }
}

fn path() -> Result<PathBuf> {
    Ok(paths()?.0)
}

fn paths() -> Result<(PathBuf, Option<PathBuf>)> {
    if let Some(path) = std::env::var_os("WUT_CONFIG").filter(|value| !value.is_empty()) {
        return Ok((PathBuf::from(path), None));
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        let (canonical, legacy) = config_paths(Path::new(&path));
        return Ok((canonical, Some(legacy)));
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            Error::new(
                "HOME is not set",
                "set XDG_CONFIG_HOME to a writable directory and try again",
            )
        })?;
    let (canonical, legacy) = config_paths(&PathBuf::from(home).join(".config"));
    Ok((canonical, Some(legacy)))
}

fn config_paths(root: &Path) -> (PathBuf, PathBuf) {
    (root.join("wut/config.json"), root.join("ask/config.json"))
}

fn invalid_config(message: impl Into<String>) -> Error {
    Error::new(message, "fix or remove the config file, then try again")
}
