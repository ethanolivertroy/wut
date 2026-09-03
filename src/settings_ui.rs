use std::io::{self, Write};

use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{Clear, ClearType};

use crate::cerebras::{self, Model};
use crate::config::{Config, ResponseColor};
use crate::error::{Error, Result};
use crate::instructions::Instructions;
use crate::select::{self, Choice, Item};

pub struct Cache;

impl Default for Cache {
    fn default() -> Self {
        Self
    }
}

pub fn run_defaults(config: &mut Config, _cache: &mut Cache) -> Result<()> {
    let _screen = select::Screen::enter()?;
    defaults_menu(config)
}

pub fn run_session(
    settings: &mut crate::Settings,
    config: &mut Config,
    _cache: &mut Cache,
) -> Result<()> {
    let _screen = select::Screen::enter()?;
    session_menu(settings, config)
}

fn defaults_menu(config: &mut Config) -> Result<()> {
    let mut selected = 0;
    loop {
        let items = [
            Item::new("Model", model_label(config.model())),
            Item::new(
                "Reasoning",
                reasoning_label(config.model(), config.reasoning()),
            ),
            Item::new("Instructions", instructions_name(config)),
            Item::new("Response background", config.response_color().name()),
            Item::new("Done", "Exit settings"),
        ];
        match select::choose(
            "Settings",
            "Defaults and answer instructions",
            &items,
            selected,
        )? {
            Choice::Selected(index) => {
                selected = index;
                match index {
                    0 => select_model(config)?,
                    1 => select_reasoning(config)?,
                    2 => select_instructions(config)?,
                    3 => select_response_color(config)?,
                    _ => return Ok(()),
                }
            }
            Choice::Cancelled => return Ok(()),
        }
    }
}

fn session_menu(settings: &mut crate::Settings, config: &mut Config) -> Result<()> {
    let mut selected = 0;
    loop {
        let items = [
            Item::new("Model", model_label(settings.model.as_deref())),
            Item::new(
                "Reasoning",
                reasoning_label(settings.model.as_deref(), settings.reasoning.as_deref()),
            ),
            Item::new("Instructions", instructions_name(config)),
            Item::new("Response background", config.response_color().name()),
            Item::new("Done", "Return to your session"),
        ];
        match select::choose(
            "Session settings",
            "Model and reasoning apply only to this session",
            &items,
            selected,
        )? {
            Choice::Selected(index) => {
                selected = index;
                match index {
                    0 => select_model(settings)?,
                    1 => select_reasoning(settings)?,
                    2 => select_instructions(config)?,
                    3 => select_response_color(config)?,
                    _ => return Ok(()),
                }
            }
            Choice::Cancelled => return Ok(()),
        }
    }
}

trait EditableSettings {
    fn model(&self) -> Option<&str>;
    fn reasoning(&self) -> Option<&str>;
    fn set_model(&mut self, model: Option<String>);
    fn set_reasoning(&mut self, reasoning: Option<String>);
    fn save(&self) -> Result<()>;
}

impl EditableSettings for Config {
    fn model(&self) -> Option<&str> {
        Config::model(self)
    }

    fn reasoning(&self) -> Option<&str> {
        Config::reasoning(self)
    }

    fn set_model(&mut self, model: Option<String>) {
        Config::set_model(self, model);
    }

    fn set_reasoning(&mut self, reasoning: Option<String>) {
        Config::set_reasoning(self, reasoning);
    }

    fn save(&self) -> Result<()> {
        Config::save(self)
    }
}

impl EditableSettings for crate::Settings {
    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    fn reasoning(&self) -> Option<&str> {
        self.reasoning.as_deref()
    }

    fn set_model(&mut self, model: Option<String>) {
        self.model = model;
    }

    fn set_reasoning(&mut self, reasoning: Option<String>) {
        self.reasoning = reasoning;
    }

    fn save(&self) -> Result<()> {
        Ok(())
    }
}

fn model_label(id: Option<&str>) -> &'static str {
    cerebras::find_model(cerebras::resolve_model(id))
        .map(|model| model.name)
        .unwrap_or("Default")
}

fn reasoning_label(model: Option<&str>, reasoning: Option<&str>) -> String {
    let model = cerebras::find_model(cerebras::resolve_model(model));
    match reasoning {
        Some(level) => title_case(level),
        None => model
            .and_then(|model| model.default_reasoning)
            .map(title_case)
            .unwrap_or_else(|| "Default".into()),
    }
}

fn instructions_name(config: &Config) -> &'static str {
    match config.instructions() {
        Instructions::Concise => "Concise",
        Instructions::Custom(_) => "Custom",
        Instructions::AgentDefault => "Agent default",
    }
}

fn select_instructions(config: &mut Config) -> Result<()> {
    let items = [
        Item::new("Concise", "Friendly, direct answers"),
        Item::new("Agent default", "Do not add wut instructions"),
        Item::new("Custom…", "Write your own instructions"),
    ];
    let selected = match config.instructions() {
        Instructions::Concise => 0,
        Instructions::AgentDefault => 1,
        Instructions::Custom(_) => 2,
    };
    let Choice::Selected(index) = select::choose(
        "Instructions",
        "How answers should be written",
        &items,
        selected,
    )?
    else {
        return Ok(());
    };

    match index {
        0 => config.set_instructions(Instructions::Concise),
        1 => config.set_instructions(Instructions::AgentDefault),
        _ => {
            let initial = config.instructions().custom().unwrap_or_default();
            let Some(instructions) = crate::prompt::edit_text("Custom instructions", initial)?
            else {
                return Ok(());
            };
            let instructions = instructions.trim();
            if instructions.is_empty() {
                show_message("Instructions", "Custom instructions cannot be empty.")?;
                return Ok(());
            }
            config.set_instructions(Instructions::Custom(instructions.to_owned()));
        }
    }
    config.save()
}

fn select_model(settings: &mut impl EditableSettings) -> Result<()> {
    let models = cerebras::MODELS;
    let items: Vec<_> = models
        .iter()
        .map(|model| Item::new(model.name, model.description))
        .collect();
    let selected = settings
        .model()
        .and_then(|current| models.iter().position(|model| model.id == current))
        .unwrap_or(0);
    let Choice::Selected(index) =
        select::choose("Model", "Cerebras public models", &items, selected)?
    else {
        return Ok(());
    };
    let model = &models[index];
    settings.set_model(Some(model.id.to_owned()));
    if !supports_reasoning(model, settings.reasoning()) {
        settings.set_reasoning(model.default_reasoning.map(str::to_owned));
    }
    settings.save()
}

fn select_reasoning(settings: &mut impl EditableSettings) -> Result<()> {
    let model = cerebras::find_model(cerebras::resolve_model(settings.model()))
        .ok_or_else(|| Error::new("no model is selected", "choose a model and try again"))?;
    let items: Vec<_> = model
        .levels
        .iter()
        .map(|level| Item::new(title_case(level), reasoning_description(level)))
        .collect();
    let selected = settings
        .reasoning()
        .and_then(|current| model.levels.iter().position(|level| *level == current))
        .unwrap_or(0);
    let Choice::Selected(index) = select::choose(
        "Reasoning",
        &format!("Levels supported by {}", model.name),
        &items,
        selected,
    )?
    else {
        return Ok(());
    };
    settings.set_reasoning(Some(model.levels[index].to_owned()));
    settings.save()
}

fn select_response_color(config: &mut Config) -> Result<()> {
    let items: Vec<_> = ResponseColor::ALL
        .iter()
        .map(|color| Item::new(color.name(), "Legible text on a colored background"))
        .collect();
    let selected = ResponseColor::ALL
        .iter()
        .position(|color| *color == config.response_color())
        .unwrap_or(0);
    let Choice::Selected(index) = select::choose(
        "Response background",
        "Background used for assistant responses",
        &items,
        selected,
    )?
    else {
        return Ok(());
    };
    config.set_response_color(ResponseColor::ALL[index]);
    config.save()
}

fn supports_reasoning(model: &Model, reasoning: Option<&str>) -> bool {
    reasoning.is_none() || model.levels.iter().any(|level| Some(*level) == reasoning)
}

fn reasoning_description(level: &str) -> &'static str {
    match level {
        "none" => "No extra thinking",
        "low" => "Low reasoning",
        "medium" => "Balanced reasoning",
        "high" => "Deep reasoning",
        _ => "",
    }
}

fn title_case(value: &str) -> String {
    let mut characters = value.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + characters.as_str())
        .unwrap_or_default()
}

fn show_message(title: &str, message: &str) -> Result<()> {
    let mut output = io::stderr();
    execute!(
        output,
        MoveTo(0, 0),
        Clear(ClearType::All),
        SetAttribute(Attribute::Bold),
        Print(title),
        SetAttribute(Attribute::Reset),
        Print(format!("\r\n\r\n{message}")),
        Print("\r\n\r\nPress any key to go back.")
    )
    .and_then(|()| output.flush())
    .map_err(|error| Error::terminal("could not draw settings", error))?;
    loop {
        if matches!(
            event::read()
                .map_err(|error| Error::terminal("could not read settings input", error))?,
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        ) {
            return Ok(());
        }
    }
}
