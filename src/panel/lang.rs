//! The control panel's languages: English as the code writes it, Italian built in, and any
//! language whose file is put in the hub's `lang` folder.
//!
//! A language file is TOML: the language's `name` as its speakers write it, and under `[text]`
//! each English text with its translation. Words in braces, such as `{name}`, are filled in by
//! the panel: they stay in the translation, wherever its sentence puts them. Text a file doesn't
//! translate is shown in English, so a file can be partial.
//!
//! The panel's own text is looked up as written (`tr!`). Text that comes from the hub itself, such
//! as an app's state or an error, is matched against the file's texts with words in braces too:
//! `"registered by {what}"` translates "registered by crafthub.exe".

use crafthub::config::Config;
use crafthub::paths;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

/// The languages that come with the panel, by code. English is the code's own.
const BUILT_IN: &[(&str, &str)] = &[("it", include_str!("../../assets/lang/it.toml"))];

/// A language file.
#[derive(Deserialize, Default)]
struct File {
    #[serde(default)]
    name: String,
    #[serde(default)]
    text: HashMap<String, String>,
}

/// A language the panel can be in: its code (`it`, the file's name) and its name (`Italiano`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Language {
    pub code: String,
    pub name: String,
}

/// A part of a text with words to fill in.
#[derive(Debug, PartialEq)]
enum Part {
    Text(String),
    /// Words to fill in, by name.
    Words(String),
}

/// A translation with words to fill in, for matching text from the hub.
struct Pattern {
    english: Vec<Part>,
    translation: String,
    /// How much of the English is fixed: the more, the surer a match.
    fixed: usize,
}

/// The language the panel is in, unless it is English.
struct Catalog {
    code: String,
    text: HashMap<String, String>,
    /// The texts with words to fill in, surest first.
    patterns: Vec<Pattern>,
    /// What matching has translated already.
    matched: Mutex<HashMap<String, String>>,
}

static CURRENT: RwLock<Option<Catalog>> = RwLock::new(None);

/// The folder where people put language files of their own: `<code>.toml`.
pub fn folder() -> PathBuf {
    paths::hub_home().join("lang")
}

fn read(code: &str) -> Option<File> {
    let built_in = BUILT_IN.iter().find(|(known, _)| *known == code).and_then(|(_, text)| toml::from_str::<File>(text).ok());
    // A file of one's own adds to the built-in one, or puts its own words in place of them.
    let own = fs::read_to_string(folder().join(format!("{code}.toml"))).ok().and_then(|text| toml::from_str::<File>(&text).ok());
    match (built_in, own) {
        (Some(mut file), Some(own)) => {
            file.text.extend(own.text);
            if !own.name.is_empty() {
                file.name = own.name;
            }
            Some(file)
        }
        (file, None) | (None, file) => file,
    }
}

/// The languages there are: English, the built-in ones, and the files in [`folder`].
pub fn available() -> Vec<Language> {
    let mut codes: Vec<String> = BUILT_IN.iter().map(|(code, _)| code.to_string()).collect();
    for entry in fs::read_dir(folder()).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "toml") {
            if let Some(code) = path.file_stem().map(|stem| stem.to_string_lossy().to_lowercase()) {
                if code != "en" && !codes.contains(&code) {
                    codes.push(code);
                }
            }
        }
    }
    let mut languages: Vec<Language> = codes
        .into_iter()
        .filter_map(|code| read(&code).map(|file| Language { name: if file.name.is_empty() { code.clone() } else { file.name }, code }))
        .collect();
    languages.sort_by_key(|language| language.name.to_lowercase());
    languages.insert(0, Language { code: "en".to_string(), name: "English".to_string() });
    languages
}

/// The language to start in: `CRAFTHUB_LANG`, else the one chosen in the panel (`language` in the
/// hub's settings), else the system's first language there is a translation for, else English.
pub fn initial() -> String {
    let chosen = std::env::var("CRAFTHUB_LANG").ok().filter(|code| !code.is_empty()).or_else(|| Config::load().language);
    if let Some(code) = chosen {
        return short(&code);
    }
    let there: Vec<String> = available().into_iter().map(|language| language.code).collect();
    sys_locale::get_locales().map(|locale| short(&locale)).find(|code| there.contains(code)).unwrap_or_else(|| "en".to_string())
}

/// `it-IT`, `it_IT.UTF-8` and `IT` are all `it`.
fn short(locale: &str) -> String {
    locale.split(['-', '_', '.']).next().unwrap_or_default().to_lowercase()
}

/// Puts the panel in the language with this code; English when there is no such language.
pub fn choose(code: &str) {
    let catalog = read(code).filter(|_| code != "en").map(|file| {
        let mut patterns: Vec<Pattern> = file
            .text
            .iter()
            .filter(|(english, _)| english.contains('{'))
            .map(|(english, translation)| {
                let english = parts(english);
                let fixed = english.iter().map(|part| if let Part::Text(text) = part { text.chars().count() } else { 0 }).sum();
                Pattern { english, translation: translation.clone(), fixed }
            })
            // A text that is only words to fill in would match anything.
            .filter(|pattern| pattern.fixed >= 2)
            .collect();
        patterns.sort_by_key(|pattern| std::cmp::Reverse(pattern.fixed));
        Catalog { code: code.to_string(), text: file.text, patterns, matched: Mutex::new(HashMap::new()) }
    });
    *CURRENT.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = catalog;
}

/// The code of the language the panel is in.
pub fn current() -> String {
    CURRENT.read().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref().map_or_else(|| "en".to_string(), |catalog| catalog.code.clone())
}

/// The panel's own text, in its language.
pub fn text(english: &str) -> String {
    let current = CURRENT.read().unwrap_or_else(|poisoned| poisoned.into_inner());
    match current.as_ref().and_then(|catalog| catalog.text.get(english)) {
        Some(translation) if !translation.is_empty() => translation.clone(),
        _ => english.to_string(),
    }
}

/// Fills in the words in braces.
pub fn fill(text: &str, words: &[(&str, &dyn Display)]) -> String {
    let mut filled = text.to_string();
    for (name, value) in words {
        filled = filled.replace(&format!("{{{name}}}"), &value.to_string());
    }
    filled
}

/// Text from the hub, in the panel's language: translated as it is, or by a translation with
/// words to fill in that it matches. As it was when there is none.
pub fn any(english: &str) -> String {
    let current = CURRENT.read().unwrap_or_else(|poisoned| poisoned.into_inner());
    match current.as_ref() {
        Some(catalog) => catalog.any(english, 0),
        None => english.to_string(),
    }
}

impl Catalog {
    fn any(&self, english: &str, depth: usize) -> String {
        if let Some(translation) = self.text.get(english).filter(|translation| !translation.is_empty()) {
            return translation.clone();
        }
        if english.is_empty() || depth > 2 {
            return english.to_string();
        }
        if let Some(done) = self.matched.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(english) {
            return done.clone();
        }
        let translated = self
            .patterns
            .iter()
            .find_map(|pattern| {
                let words = capture(&pattern.english, english)?;
                // What fills the words may have a translation of its own: "used by another program".
                let words: Vec<(&str, String)> = words.into_iter().map(|(name, value)| (name, self.any(value, depth + 1))).collect();
                let words: Vec<(&str, &dyn Display)> = words.iter().map(|(name, value)| (*name, value as &dyn Display)).collect();
                Some(fill(&pattern.translation, &words))
            })
            .unwrap_or_else(|| english.to_string());
        let mut matched = self.matched.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // Text with numbers in it is endless: start over now and then.
        if matched.len() > 4096 {
            matched.clear();
        }
        matched.insert(english.to_string(), translated.clone());
        translated
    }
}

/// A text cut into what is fixed and the words to fill in.
fn parts(text: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|close| open + close) else { break };
        if open > 0 {
            parts.push(Part::Text(rest[..open].to_string()));
        }
        parts.push(Part::Words(rest[open + 1..close].to_string()));
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest.to_string()));
    }
    parts
}

/// What fills each word of `parts` in `text`, if `text` reads like it.
fn capture<'a, 'p>(parts: &'p [Part], text: &'a str) -> Option<Vec<(&'p str, &'a str)>> {
    let mut rest = text;
    let mut words = Vec::new();
    let mut open: Option<&str> = None;
    for part in parts {
        match part {
            Part::Text(fixed) => match open.take() {
                None => rest = rest.strip_prefix(fixed.as_str())?,
                Some(name) => {
                    let at = rest.find(fixed.as_str()).filter(|at| *at > 0)?;
                    words.push((name, &rest[..at]));
                    rest = &rest[at + fixed.len()..];
                }
            },
            // Two words in a row can't be told apart.
            Part::Words(_) if open.is_some() => return None,
            Part::Words(name) => open = Some(name),
        }
    }
    match open {
        Some(name) if !rest.is_empty() => words.push((name, rest)),
        Some(_) => return None,
        None if !rest.is_empty() => return None,
        None => {}
    }
    Some(words)
}

/// The words in braces in a text, sorted.
#[cfg(test)]
fn words_in(text: &str) -> Vec<String> {
    let mut words: Vec<String> = parts(text).into_iter().filter_map(|part| if let Part::Words(name) = part { Some(name) } else { None }).collect();
    words.sort();
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn italian() -> File {
        toml::from_str(BUILT_IN[0].1).expect("assets/lang/it.toml is valid TOML")
    }

    #[test]
    fn matches_text_with_words_in_it() {
        let parts = parts("registered by {what}");
        assert_eq!(capture(&parts, "registered by crafthub.exe"), Some(vec![("what", "crafthub.exe")]));
        assert_eq!(capture(&parts, "registered by "), None);
        assert_eq!(capture(&parts, "registered"), None);
        let parts = super::parts("Port {port} is already used by {who}.");
        assert_eq!(capture(&parts, "Port 7801 is already used by another program."), Some(vec![("port", "7801"), ("who", "another program")]));
        assert_eq!(capture(&parts, "Port 7801 is free."), None);
        assert_eq!(fill("{name} è accesa.", &[("name", &"PhotoCraft")]), "PhotoCraft è accesa.");
    }

    #[test]
    fn locales_are_cut_to_their_language() {
        assert_eq!(short("it-IT"), "it");
        assert_eq!(short("it_IT.UTF-8"), "it");
        assert_eq!(short("EN"), "en");
    }

    #[test]
    fn italian_fills_in_the_same_words() {
        let file = italian();
        assert_eq!(file.name, "Italiano");
        for (english, translation) in &file.text {
            assert!(!translation.is_empty(), "no Italian for {english:?}");
            assert_eq!(words_in(english), words_in(translation), "the words in braces differ: {english:?} and {translation:?}");
        }
    }

    #[test]
    fn italian_has_every_text_of_the_panel() {
        let file = italian();
        let source = include_str!("../bin/crafthub-panel.rs");
        let mut missing = Vec::new();
        for piece in source.split("tr!(\"").skip(1) {
            // The text runs to the first quote that isn't escaped.
            let Some(end) = piece.char_indices().find(|&(at, c)| c == '"' && !piece[..at].ends_with('\\')).map(|(at, _)| at) else { continue };
            let english = piece[..end].replace("\\\"", "\"");
            if !file.text.contains_key(&english) && !missing.contains(&english) {
                missing.push(english);
            }
        }
        assert!(missing.is_empty(), "assets/lang/it.toml has no translation for:\n{}", missing.join("\n"));
    }
}
