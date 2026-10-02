//! Custom fields: definitions on any content type (`content_fields`) and
//! the values entries store in `posts.meta.fields`.
//!
//! Values are checked on every entry write by
//! [`ContentFieldsService::validate_values`] — unknown keys refused, each
//! value checked against its field's kind and options, media and entry
//! ids looked up — and `required` is checked when an entry is published
//! or scheduled ([`ContentFieldsService::check_required`]).
//!
//! What is stored is normalised: empty values (`null`, `""`, `[]`) are
//! dropped rather than kept, and media and entry ids are stored as
//! decimal strings (a browser cannot hold a 64-bit id as a JSON number).
//! So "an entry holds a value for `key`" is simply "`key` is present".

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use sqlx::PgPool;
use vyasa_common::AppError;
use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_db::repo::{
    ContentFieldRow, ContentFieldUpdate, ContentFieldsRepo, MediaRepo, NewContentField, PostsRepo,
};

use super::CONTENT_LOCK;

/// Most fields one type may have.
pub const MAX_FIELDS_PER_TYPE: i64 = 64;

const MAX_LABEL: usize = 120;
const MAX_HELP: usize = 500;
const TEXT_DEFAULT_MAX: u64 = 255;
const TEXT_LIMIT: u64 = 1_000;
const TEXTAREA_DEFAULT_MAX: u64 = 10_000;
const TEXTAREA_LIMIT: u64 = 100_000;
const MAX_URL: usize = 2_048;
const MAX_CHOICES: usize = 100;
const MAX_CHOICE: usize = 100;

/// What a field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    /// One line of text. Option `max_length` (1–1000, default 255).
    Text,
    /// Several lines of plain text. Option `max_length` (1–100000,
    /// default 10000).
    Textarea,
    /// A number. Options `min`, `max`, `step` (> 0; values are `min` (or
    /// 0) plus a multiple of it).
    Number,
    /// `true` or `false`.
    Boolean,
    /// A calendar date, `YYYY-MM-DD`.
    Date,
    /// One of `choices` (strings), or a list of them with `multiple`.
    Choice,
    /// An `http(s)` link or a path on this site (see [`is_allowed_url`]).
    Url,
    /// A media library item, by id.
    Media,
    /// Another entry, by id; option `entry_type` restricts its type.
    Entry,
}

impl FieldKind {
    /// Every kind.
    pub const ALL: [FieldKind; 9] = [
        FieldKind::Text,
        FieldKind::Textarea,
        FieldKind::Number,
        FieldKind::Boolean,
        FieldKind::Date,
        FieldKind::Choice,
        FieldKind::Url,
        FieldKind::Media,
        FieldKind::Entry,
    ];

    /// Parses the stored and wire form.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for anything else.
    pub fn parse(value: &str) -> Result<Self, AppError> {
        Self::ALL
            .into_iter()
            .find(|k| k.as_str() == value)
            .ok_or_else(|| {
                AppError::validation(format!(
                    "unknown field kind {value:?}: text, textarea, number, boolean, date, \
                     choice, url, media or entry"
                ))
            })
    }

    /// The stored and wire form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            FieldKind::Text => "text",
            FieldKind::Textarea => "textarea",
            FieldKind::Number => "number",
            FieldKind::Boolean => "boolean",
            FieldKind::Date => "date",
            FieldKind::Choice => "choice",
            FieldKind::Url => "url",
            FieldKind::Media => "media",
            FieldKind::Entry => "entry",
        }
    }
}

/// A kind's options, parsed.
#[derive(Clone, Debug, PartialEq)]
enum Rules {
    Text {
        max: u64,
    },
    Textarea {
        max: u64,
    },
    Number {
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
    },
    Boolean,
    Date,
    Choice {
        choices: Vec<String>,
        multiple: bool,
    },
    Url,
    Media,
    Entry {
        entry_type: Option<String>,
    },
}

/// A field definition in the form values are checked against.
#[derive(Clone, Debug)]
pub struct FieldDef {
    /// Key in `meta.fields`.
    pub key: String,
    /// Label, for messages.
    pub label: String,
    /// Kind.
    pub kind: FieldKind,
    /// Whether publishing or scheduling needs a value.
    pub required: bool,
    rules: Rules,
}

impl FieldDef {
    /// Reads a stored definition.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for a kind or options the field
    /// rules refuse (which the service never stores).
    pub fn from_row(row: &ContentFieldRow) -> Result<Self, AppError> {
        let kind = FieldKind::parse(&row.kind)?;
        Ok(Self {
            key: row.key.clone(),
            label: row.label.clone(),
            kind,
            required: row.required,
            rules: parse_options(kind, &row.options)?,
        })
    }

    /// Checks one value against the field's kind and options, returning
    /// it as it is stored, or `None` for no value (`null`, `""`, `[]`).
    ///
    /// Shape only: whether a media item or entry id names something that
    /// exists needs the database, and is checked by
    /// [`ContentFieldsService::validate_values`].
    ///
    /// # Errors
    ///
    /// A sentence for the author, without the field's name.
    pub fn check(&self, value: &Value) -> Result<Option<Value>, String> {
        if is_empty(value) {
            return Ok(None);
        }
        let stored = match &self.rules {
            Rules::Text { max } => Value::String(check_text(value, *max, false)?),
            Rules::Textarea { max } => Value::String(check_text(value, *max, true)?),
            Rules::Number { min, max, step } => check_number(value, *min, *max, *step)?,
            Rules::Boolean => match value {
                Value::Bool(_) => value.clone(),
                _ => return Err("must be true or false".to_owned()),
            },
            Rules::Date => match value.as_str() {
                Some(s) if is_iso_date(s) => value.clone(),
                _ => return Err("must be a date written YYYY-MM-DD".to_owned()),
            },
            Rules::Choice { choices, multiple } => check_choice(value, choices, *multiple)?,
            Rules::Url => match value.as_str() {
                Some(s) if is_allowed_url(s) => value.clone(),
                _ => {
                    return Err(format!(
                        "must be an http:// or https:// link, or a path on this site \
                         starting with a single / (at most {MAX_URL} characters, no spaces)"
                    ))
                }
            },
            Rules::Media | Rules::Entry { .. } => Value::String(parse_id(value)?.to_string()),
        };
        Ok(Some(stored))
    }
}

/// Whether `value` is no value at all.
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

fn check_text(value: &Value, max: u64, multiline: bool) -> Result<String, String> {
    let Some(s) = value.as_str() else {
        return Err("must be text".to_owned());
    };
    if u64::try_from(s.chars().count()).unwrap_or(u64::MAX) > max {
        return Err(format!("must be at most {max} characters"));
    }
    let allowed = |c: char| multiline && matches!(c, '\n' | '\r' | '\t');
    if s.chars().any(|c| c.is_control() && !allowed(c)) {
        return Err(if multiline {
            "must not contain control characters".to_owned()
        } else {
            "must be one line, without line breaks or control characters".to_owned()
        });
    }
    Ok(s.to_owned())
}

fn check_number(
    value: &Value,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
) -> Result<Value, String> {
    let Some(n) = value.as_f64().filter(|n| n.is_finite()) else {
        return Err("must be a number".to_owned());
    };
    if let Some(min) = min {
        if n < min {
            return Err(format!("must be at least {min}"));
        }
    }
    if let Some(max) = max {
        if n > max {
            return Err(format!("must be at most {max}"));
        }
    }
    if let Some(step) = step {
        let base = min.unwrap_or(0.0);
        let steps = (n - base) / step;
        if (steps - steps.round()).abs() > 1e-9 * steps.abs().max(1.0) {
            return Err(if base == 0.0 {
                format!("must be a multiple of {step}")
            } else {
                format!("must be {base} plus a multiple of {step}")
            });
        }
    }
    Ok(value.clone())
}

fn check_choice(value: &Value, choices: &[String], multiple: bool) -> Result<Value, String> {
    let one_of = || format!("must be one of: {}", choices.join(", "));
    if !multiple {
        return match value.as_str() {
            Some(s) if choices.iter().any(|c| c == s) => Ok(value.clone()),
            _ => Err(one_of()),
        };
    }
    let Some(items) = value.as_array() else {
        return Err(format!(
            "must be a list of choices from: {}",
            choices.join(", ")
        ));
    };
    let mut seen = BTreeSet::new();
    for item in items {
        let Some(s) = item.as_str().filter(|s| choices.iter().any(|c| c == s)) else {
            return Err(format!("{item} is not a choice; {}", one_of()));
        };
        if !seen.insert(s) {
            return Err(format!("lists {s:?} more than once"));
        }
    }
    Ok(value.clone())
}

/// A positive 64-bit id, from a JSON number or a decimal string.
fn parse_id(value: &Value) -> Result<i64, String> {
    let id = match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s)
            if !s.is_empty() && s.len() <= 19 && s.bytes().all(|b| b.is_ascii_digit()) =>
        {
            s.parse::<i64>().ok()
        }
        _ => None,
    };
    id.filter(|id| *id > 0)
        .ok_or_else(|| "must be an id (a positive whole number)".to_owned())
}

/// `YYYY-MM-DD` naming a real calendar date.
fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || ![0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|&i| bytes[i].is_ascii_digit())
    {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        value[0..4].parse(),
        value[5..7].parse(),
        value[8..10].parse(),
    ) else {
        return false;
    };
    chrono::NaiveDate::from_ymd_opt(year, month, day).is_some()
}

/// A path on this site: starts with a single `/`, and is not a
/// network-path reference a browser would take to another host. Browsers
/// read a backslash as a slash, so `/\host` is refused like `//host`, and
/// drop tab, CR and LF from a URL before reading it, so no control
/// character or whitespace is allowed anywhere (`/<tab>/host` is
/// `//host`). The same rule as the redirect validator's.
#[must_use]
pub fn is_site_relative_url(value: &str) -> bool {
    value.starts_with('/')
        && !value.starts_with("//")
        && !value.starts_with("/\\")
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Characters a link has no business holding because they make it read
/// as something it is not:
///
/// - every Unicode format character (general category `Cf`, Unicode 15.1:
///   soft hyphen, Arabic number signs, zero-width and joiner characters,
///   bidirectional controls, invisible operators, the byte-order mark,
///   interlinear annotation, shorthand and musical format controls, tags);
/// - invisible characters that are not `Cf`: the combining grapheme
///   joiner, Hangul fillers, the Braille blank, Mongolian and other
///   variation selectors, and the whole tag block;
/// - look-alikes of `/`, `\` and `:` (fullwidth, small, mathematical and
///   box-drawing forms, and strokes and radicals drawn as a slash), which
///   some parsers fold into the real ones and every reader confuses.
fn is_deceptive(c: char) -> bool {
    matches!(
        c,
        // Cf.
        '\u{ad}'
            | '\u{600}'..='\u{605}'
            | '\u{61c}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{890}'..='\u{891}'
            | '\u{8e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            // The tag block (Cf, plus its unassigned first code point).
            | '\u{e0000}'..='\u{e007f}'
            // Invisible but not Cf.
            | '\u{34f}'
            | '\u{115f}'..='\u{1160}'
            | '\u{180b}'..='\u{180d}'
            | '\u{180f}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{ffa0}'
            | '\u{e0100}'..='\u{e01ef}'
            // Slash look-alikes.
            | '\u{1735}'
            | '\u{2041}'
            | '\u{2044}'
            | '\u{2215}'
            | '\u{2571}'
            | '\u{27cb}'
            | '\u{29f8}'
            | '\u{2f03}'
            | '\u{31d3}'
            | '\u{ff0f}'
            // Backslash look-alikes.
            | '\u{2216}'
            | '\u{2572}'
            | '\u{27cd}'
            | '\u{29f5}'
            | '\u{29f9}'
            | '\u{31d4}'
            | '\u{fe68}'
            | '\u{ff3c}'
            // Colon look-alikes.
            | '\u{ff1a}'
            | '\u{fe55}'
            | '\u{a789}'
    )
}

/// What a `url` field may hold: an `http://` or `https://` link with a
/// host, or a path on this site ([`is_site_relative_url`]); at most 2048
/// characters, with no control character, whitespace, slash or colon
/// look-alike, or invisible character anywhere. Schemes are compared
/// ASCII-case-insensitively, so `JaVaScRiPt:` is refused like any other
/// scheme that is not http(s).
#[must_use]
pub fn is_allowed_url(value: &str) -> bool {
    if value.len() > MAX_URL || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    if value.chars().any(is_deceptive) {
        return false;
    }
    if is_site_relative_url(value) {
        return true;
    }
    let lower = value.get(..8).unwrap_or(value).to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &value[8..]
    } else if lower.starts_with("http://") {
        &value[7..]
    } else {
        return false;
    };
    !rest.is_empty() && !rest.starts_with(['/', '\\'])
}

/// The option keys each kind takes.
fn option_keys(kind: FieldKind) -> &'static [&'static str] {
    match kind {
        FieldKind::Text | FieldKind::Textarea => &["max_length"],
        FieldKind::Number => &["min", "max", "step"],
        FieldKind::Choice => &["choices", "multiple"],
        FieldKind::Entry => &["entry_type"],
        FieldKind::Boolean | FieldKind::Date | FieldKind::Url | FieldKind::Media => &[],
    }
}

/// Parses a kind's options; `null` counts as absent.
fn parse_options(kind: FieldKind, options: &Value) -> Result<Rules, AppError> {
    let Some(map) = options.as_object() else {
        return Err(AppError::validation(
            "a field's options must be a JSON object",
        ));
    };
    let allowed = option_keys(kind);
    if let Some(unknown) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(AppError::validation(format!(
            "the option {unknown:?} does not apply to a {} field",
            kind.as_str()
        )));
    }
    let get = |key: &str| map.get(key).filter(|v| !v.is_null());
    let number = |key: &str| -> Result<Option<f64>, AppError> {
        get(key)
            .map(|v| {
                v.as_f64().filter(|n| n.is_finite()).ok_or_else(|| {
                    AppError::validation(format!("the option {key:?} must be a number"))
                })
            })
            .transpose()
    };
    let max_length = |default: u64, limit: u64| -> Result<u64, AppError> {
        match get("max_length") {
            None => Ok(default),
            Some(v) => v
                .as_u64()
                .filter(|n| (1..=limit).contains(n))
                .ok_or_else(|| {
                    AppError::validation(format!(
                        "the option \"max_length\" must be a whole number from 1 to {limit}"
                    ))
                }),
        }
    };
    Ok(match kind {
        FieldKind::Text => Rules::Text {
            max: max_length(TEXT_DEFAULT_MAX, TEXT_LIMIT)?,
        },
        FieldKind::Textarea => Rules::Textarea {
            max: max_length(TEXTAREA_DEFAULT_MAX, TEXTAREA_LIMIT)?,
        },
        FieldKind::Number => {
            let (min, max, step) = (number("min")?, number("max")?, number("step")?);
            if let (Some(min), Some(max)) = (min, max) {
                if min > max {
                    return Err(AppError::validation(
                        "the option \"min\" must not be greater than \"max\"",
                    ));
                }
            }
            if step.is_some_and(|s| s <= 0.0) {
                return Err(AppError::validation(
                    "the option \"step\" must be greater than 0",
                ));
            }
            Rules::Number { min, max, step }
        }
        FieldKind::Boolean => Rules::Boolean,
        FieldKind::Date => Rules::Date,
        FieldKind::Url => Rules::Url,
        FieldKind::Media => Rules::Media,
        FieldKind::Choice => {
            let multiple = match get("multiple") {
                None => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => {
                    return Err(AppError::validation(
                        "the option \"multiple\" must be true or false",
                    ))
                }
            };
            Rules::Choice {
                choices: parse_choices(get("choices"))?,
                multiple,
            }
        }
        FieldKind::Entry => Rules::Entry {
            entry_type: match get("entry_type") {
                None => None,
                Some(Value::String(t)) if t != "block" => Some(t.clone()),
                Some(_) => {
                    return Err(AppError::validation(
                        "the option \"entry_type\" must name a content type",
                    ))
                }
            },
        },
    })
}

fn parse_choices(value: Option<&Value>) -> Result<Vec<String>, AppError> {
    let bad = || {
        AppError::validation(format!(
            "a choice field needs \"choices\": 1 to {MAX_CHOICES} different, non-empty \
             strings of at most {MAX_CHOICE} characters"
        ))
    };
    let items = value.and_then(Value::as_array).ok_or_else(bad)?;
    if items.is_empty() || items.len() > MAX_CHOICES {
        return Err(bad());
    }
    let mut choices: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let s = item.as_str().ok_or_else(bad)?;
        if s.trim().is_empty()
            || s.chars().count() > MAX_CHOICE
            || s.chars().any(char::is_control)
            || choices.iter().any(|c| c == s)
        {
            return Err(bad());
        }
        choices.push(s.to_owned());
    }
    Ok(choices)
}

/// Checks options for a definition being written, including that an
/// entry field's `entry_type` names a type that exists now.
fn check_options(kind: FieldKind, options: &Value) -> Result<Value, AppError> {
    if let Rules::Entry {
        entry_type: Some(t),
    } = parse_options(kind, options)?
    {
        if PostType::parse(&t).is_err() {
            return Err(AppError::validation(format!(
                "the option \"entry_type\" names no content type: {t:?}"
            )));
        }
    }
    Ok(options.clone())
}

/// A key: `^[a-z][a-z0-9_]{0,39}$`.
fn check_field_key(key: &str) -> Result<(), AppError> {
    let ok = (1..=40).contains(&key.len())
        && key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(AppError::validation(format!(
            "a field key is 1 to 40 characters: lowercase letters, digits and underscores, \
             starting with a letter (got {key:?})"
        )))
    }
}

fn label(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::validation("a field's label must not be empty"));
    }
    if value.chars().count() > MAX_LABEL || value.chars().any(char::is_control) {
        return Err(AppError::validation(format!(
            "a field's label is at most {MAX_LABEL} characters, without control characters"
        )));
    }
    Ok(value.to_owned())
}

fn help(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.chars().count() > MAX_HELP || value.chars().any(|c| c.is_control() && c != '\n') {
        return Err(AppError::validation(format!(
            "a field's help text is at most {MAX_HELP} characters, without control characters"
        )));
    }
    Ok(value.to_owned())
}

/// A field to create.
#[derive(Clone, Debug)]
pub struct NewField {
    /// Key in `meta.fields`: `^[a-z][a-z0-9_]{0,39}$`; cannot change later.
    pub key: String,
    /// Label shown in the editor.
    pub label: String,
    /// Help text shown under the input (may be empty).
    pub help: String,
    /// Kind.
    pub kind: FieldKind,
    /// Whether publishing or scheduling needs a value.
    pub required: bool,
    /// Per-kind options (a JSON object; see [`FieldKind`]).
    pub options: Value,
}

/// Changes to a field; `None` keeps a value. The key never changes.
#[derive(Clone, Debug, Default)]
pub struct FieldChanges {
    /// New label.
    pub label: Option<String>,
    /// New help text.
    pub help: Option<String>,
    /// New kind; refused while any entry holds a value for the field.
    /// Without new `options` the options reset to the new kind's defaults.
    pub kind: Option<FieldKind>,
    /// New required flag.
    pub required: Option<bool>,
    /// New options, replacing the old ones.
    pub options: Option<Value>,
}

/// Field definitions and the values entries store.
#[derive(Clone, Debug)]
pub struct ContentFieldsService {
    fields: ContentFieldsRepo,
    posts: PostsRepo,
    media: MediaRepo,
}

/// A type that may carry fields: `post`, `page` or a live custom type.
fn fieldable(type_slug: &str) -> Result<(), AppError> {
    match type_slug {
        "post" | "page" => Ok(()),
        "block" => Err(AppError::validation("reusable blocks do not take fields")),
        other => PostType::parse(other)
            .map(|_| ())
            .map_err(|_| AppError::not_found("content_type", other)),
    }
}

impl ContentFieldsService {
    /// Creates the service over `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            fields: ContentFieldsRepo::new(pool.clone()),
            posts: PostsRepo::new(pool.clone()),
            media: MediaRepo::new(pool),
        }
    }

    /// A type's fields in order.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] for a type that does not exist,
    /// [`AppError::Validation`] for `block`.
    pub async fn list(&self, type_slug: &str) -> Result<Vec<ContentFieldRow>, AppError> {
        fieldable(type_slug)?;
        self.fields.list(type_slug).await
    }

    /// One field.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] (`content_field`) when missing.
    pub async fn get(&self, type_slug: &str, key: &str) -> Result<ContentFieldRow, AppError> {
        self.fields.get(type_slug, key).await
    }

    /// A type's fields in order, ready to check values against (empty for
    /// a type without fields).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn definitions(&self, post_type: PostType) -> Result<Vec<FieldDef>, AppError> {
        self.fields
            .list(post_type.as_str())
            .await?
            .iter()
            .map(|row| {
                // The service never stores options the rules refuse, so a
                // failure here is a broken row, not bad input.
                FieldDef::from_row(row).map_err(|e| {
                    AppError::internal_msg(format!(
                        "stored field {}.{} is unreadable: {e}",
                        row.type_slug, row.key
                    ))
                })
            })
            .collect()
    }

    /// Adds a field after the type's last one.
    ///
    /// Review focus 4: a key whose deleted field's values are still stored
    /// in entries is refused until they are cleaned up
    /// ([`ContentFieldsService::clean_up`]), whatever the new kind — old
    /// values are never silently adopted by a new definition.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for a bad key, label, help text or
    /// options, `block`, or a type at [`MAX_FIELDS_PER_TYPE`];
    /// [`AppError::NotFound`] for a type that does not exist;
    /// [`AppError::Conflict`] when the key is taken or still has stored
    /// values.
    pub async fn create(
        &self,
        type_slug: &str,
        input: NewField,
    ) -> Result<ContentFieldRow, AppError> {
        check_field_key(&input.key)?;
        let label = label(&input.label)?;
        let help = help(&input.help)?;
        let options = check_options(input.kind, &input.options)?;
        let key = input.key.as_str();
        // The type's delete takes the same lock, so the type checked here
        // is still there when the row is written: no field outlives it.
        let _turn = CONTENT_LOCK.lock().await;
        fieldable(type_slug)?;
        match self.fields.get(type_slug, key).await {
            Ok(_) => {
                return Err(AppError::conflict(format!(
                    "the type {type_slug:?} already has a field {key:?}"
                )))
            }
            Err(AppError::NotFound { .. }) => {}
            Err(other) => return Err(other),
        }
        if self.fields.count(type_slug).await? >= MAX_FIELDS_PER_TYPE {
            return Err(AppError::validation(format!(
                "a type may have at most {MAX_FIELDS_PER_TYPE} fields"
            )));
        }
        let holding = self.fields.count_entries_with_value(type_slug, key).await?;
        if holding > 0 {
            return Err(AppError::conflict(format!(
                "{holding} {} of {type_slug:?} still hold values for {key:?} from a field that \
                 was deleted; clean them up before reusing the key",
                entries(holding)
            )));
        }
        self.fields
            .insert(&NewContentField {
                type_slug,
                key,
                label: &label,
                help: &help,
                kind: input.kind.as_str(),
                required: input.required,
                options,
            })
            .await
    }

    /// Changes a field. Its kind cannot change while any entry (in any
    /// status) holds a value for it.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing,
    /// [`AppError::Validation`] for a bad label, help text or options,
    /// [`AppError::Conflict`] for a kind change while values exist.
    pub async fn update(
        &self,
        type_slug: &str,
        key: &str,
        changes: FieldChanges,
    ) -> Result<ContentFieldRow, AppError> {
        let current = self.fields.get(type_slug, key).await?;
        let current_kind = FieldKind::parse(&current.kind)?;
        let kind = changes.kind.unwrap_or(current_kind);
        let kind_changes = kind != current_kind;
        let options = match changes.options {
            Some(options) => Some(check_options(kind, &options)?),
            None if kind_changes => Some(check_options(kind, &Value::Object(Map::new()))?),
            None => None,
        };
        let label = changes.label.as_deref().map(label).transpose()?;
        let help = changes.help.as_deref().map(help).transpose()?;
        if kind_changes {
            let holding = self.fields.count_entries_with_value(type_slug, key).await?;
            if holding > 0 {
                return Err(AppError::conflict(format!(
                    "{holding} {} of {type_slug:?} hold a value for {key:?}; its kind cannot \
                     change while any does",
                    entries(holding)
                )));
            }
        }
        self.fields
            .update(
                type_slug,
                key,
                &ContentFieldUpdate {
                    label: label.as_deref(),
                    help: help.as_deref(),
                    kind: kind_changes.then(|| kind.as_str()),
                    required: changes.required,
                    options: options.as_ref(),
                },
            )
            .await
    }

    /// Deletes a field definition. Values entries hold for it stay (they
    /// are no longer served or validated) until
    /// [`ContentFieldsService::clean_up`] removes them.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn delete(&self, type_slug: &str, key: &str) -> Result<(), AppError> {
        if self.fields.delete(type_slug, key).await? {
            Ok(())
        } else {
            Err(AppError::not_found(
                "content_field",
                format!("{type_slug}.{key}"),
            ))
        }
    }

    /// Puts a type's fields in the order of `keys`, which must name each
    /// of them exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when `keys` is not exactly the
    /// type's keys, [`AppError::NotFound`] for a type that does not exist.
    pub async fn reorder(
        &self,
        type_slug: &str,
        keys: &[String],
    ) -> Result<Vec<ContentFieldRow>, AppError> {
        let current: BTreeSet<String> = self
            .list(type_slug)
            .await?
            .into_iter()
            .map(|f| f.key)
            .collect();
        let given: BTreeSet<String> = keys.iter().cloned().collect();
        if given.len() != keys.len() || given != current {
            return Err(AppError::validation(
                "list every field of the type exactly once, in the new order",
            ));
        }
        self.fields.reorder(type_slug, keys).await
    }

    /// Removes the stored values of a deleted field from every entry of
    /// the type (any status) and from their revisions and autosaves.
    /// Returns how many entries changed.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for a malformed key,
    /// [`AppError::Conflict`] while the field is still defined.
    pub async fn clean_up(&self, type_slug: &str, key: &str) -> Result<u64, AppError> {
        check_field_key(key)?;
        match self.fields.get(type_slug, key).await {
            Ok(_) => {
                return Err(AppError::conflict(format!(
                    "{key:?} is still a field of {type_slug:?}; delete it before cleaning up \
                     its values"
                )))
            }
            Err(AppError::NotFound { .. }) => {}
            Err(other) => return Err(other),
        }
        self.fields.clear_values(type_slug, key).await
    }

    /// Keys whose values entries of the type still store but which no
    /// field defines any more, with how many entries hold one — what
    /// [`ContentFieldsService::clean_up`] can remove.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn orphaned_values(&self, type_slug: &str) -> Result<Vec<(String, i64)>, AppError> {
        let defined: BTreeSet<String> = self
            .fields
            .list(type_slug)
            .await?
            .into_iter()
            .map(|f| f.key)
            .collect();
        Ok(self
            .fields
            .stored_value_counts(type_slug)
            .await?
            .into_iter()
            .filter(|(key, _)| !defined.contains(key))
            .collect())
    }

    /// Checks field values for an entry of `post_type` and returns them as
    /// they are stored: unknown keys refused, every value checked against
    /// its field, empty values dropped, ids as strings.
    ///
    /// `stored` is what the entry holds now (`None` for a new entry). A
    /// media or entry reference equal to the stored one is not looked up
    /// again, so a reference to something deleted since does not block
    /// saving the entry's other changes; a new or changed one must exist
    /// (and an entry must be of the allowed type, and not in the trash).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] listing every problem, each as
    /// `fields.<key>: <message>`, separated by `; `.
    pub async fn validate_values(
        &self,
        post_type: PostType,
        raw: &Value,
        stored: Option<&Value>,
    ) -> Result<Map<String, Value>, AppError> {
        let Some(input) = raw.as_object() else {
            return Err(AppError::validation(
                "fields must be a JSON object keyed by field key",
            ));
        };
        let mut out = Map::new();
        if input.is_empty() {
            return Ok(out);
        }
        let defs = self.definitions(post_type).await?;
        let by_key: BTreeMap<&str, &FieldDef> = defs.iter().map(|d| (d.key.as_str(), d)).collect();
        let stored = stored.and_then(Value::as_object);
        let mut errors = Vec::new();
        for (key, value) in input {
            let Some(def) = by_key.get(key.as_str()) else {
                errors.push(format!(
                    "fields.{key}: {} has no such field",
                    post_type.as_str()
                ));
                continue;
            };
            match def.check(value) {
                Err(message) => errors.push(format!("fields.{key}: {message}")),
                Ok(None) => {}
                Ok(Some(value)) => {
                    let unchanged = stored.and_then(|s| s.get(key)) == Some(&value);
                    if !unchanged {
                        if let Some(message) = self.missing_reference(def, &value).await? {
                            errors.push(format!("fields.{key}: {message}"));
                            continue;
                        }
                    }
                    out.insert(key.clone(), value);
                }
            }
        }
        if errors.is_empty() {
            Ok(out)
        } else {
            Err(AppError::validation(errors.join("; ")))
        }
    }

    /// `values` (the checked values of defined fields) plus whatever
    /// `stored` holds under keys no field of `post_type` defines any more:
    /// a deleted field's values stay stored until
    /// [`ContentFieldsService::clean_up`] removes them, so a save that
    /// sends the defined fields never wipes them.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn keep_orphans(
        &self,
        post_type: PostType,
        mut values: Map<String, Value>,
        stored: Option<&Value>,
    ) -> Result<Map<String, Value>, AppError> {
        let Some(stored) = stored.and_then(Value::as_object).filter(|m| !m.is_empty()) else {
            return Ok(values);
        };
        let defs = self.definitions(post_type).await?;
        for (key, value) in stored {
            if !defs.iter().any(|d| &d.key == key) {
                values.insert(key.clone(), value.clone());
            }
        }
        Ok(values)
    }

    /// Why a checked media or entry value does not name something it may,
    /// if it does not.
    async fn missing_reference(
        &self,
        def: &FieldDef,
        value: &Value,
    ) -> Result<Option<String>, AppError> {
        let Ok(id) = parse_id(value) else {
            return Ok(None);
        };
        match &def.rules {
            Rules::Media => Ok(match self.media.trash_state(id).await? {
                Some(false) => None,
                Some(true) => Some(format!("media item {id} is in the trash")),
                None => Some(format!("there is no media item {id}")),
            }),
            Rules::Entry { entry_type } => match self.posts.get(id).await {
                Ok(row) if row.status == PostStatus::Trash => {
                    Ok(Some(format!("entry {id} is in the trash")))
                }
                Ok(row) if row.post_type == PostType::Block => {
                    Ok(Some(format!("{id} is a reusable block, not an entry")))
                }
                Ok(row) => Ok(entry_type
                    .as_deref()
                    .filter(|t| *t != row.post_type.as_str())
                    .map(|t| {
                        format!(
                            "entry {id} is a {}, and this field takes a {t}",
                            row.post_type.as_str()
                        )
                    })),
                Err(AppError::NotFound { .. }) => Ok(Some(format!("there is no entry {id}"))),
                Err(e) => Err(e),
            },
            _ => Ok(None),
        }
    }

    /// Checks that every required field of `post_type` has a value in
    /// `values` — what publishing or scheduling an entry needs.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] listing each missing field as
    /// `fields.<key>: <label> is required to publish or schedule`.
    pub async fn check_required(
        &self,
        post_type: PostType,
        values: &Map<String, Value>,
    ) -> Result<(), AppError> {
        let missing: Vec<String> = self
            .definitions(post_type)
            .await?
            .into_iter()
            .filter(|d| d.required && values.get(&d.key).is_none_or(is_empty))
            .map(|d| {
                format!(
                    "fields.{}: {} is required to publish or schedule",
                    d.key, d.label
                )
            })
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(AppError::validation(missing.join("; ")))
        }
    }

    /// The values of a revision being restored, as far as they still fit,
    /// and the keys of those dropped.
    ///
    /// A value is dropped when its field is gone, the field no longer
    /// takes it (its kind or options changed since), or — for a value
    /// that differs from the entry's `current` one — it names a media item
    /// or entry it may not (missing, in the trash, a block, or of the
    /// wrong type): a media id restored into a field that now takes
    /// entries must not become a reference to whatever post has that id.
    /// A value equal to the current one is kept unchecked, as on any save.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn restorable_values(
        &self,
        post_type: PostType,
        raw: &Value,
        current: Option<&Value>,
    ) -> Result<(Map<String, Value>, Vec<String>), AppError> {
        let mut kept = Map::new();
        let mut dropped = Vec::new();
        let Some(input) = raw.as_object().filter(|m| !m.is_empty()) else {
            return Ok((kept, dropped));
        };
        let defs = self.definitions(post_type).await?;
        let current = current.and_then(Value::as_object);
        for (key, value) in input {
            let Some(def) = defs.iter().find(|d| &d.key == key) else {
                dropped.push(key.clone());
                continue;
            };
            match def.check(value) {
                Ok(None) => {}
                Ok(Some(value)) => {
                    let unchanged = current.and_then(|c| c.get(key)) == Some(&value);
                    if !unchanged && self.missing_reference(def, &value).await?.is_some() {
                        dropped.push(key.clone());
                    } else {
                        kept.insert(key.clone(), value);
                    }
                }
                Err(_) => dropped.push(key.clone()),
            }
        }
        Ok((kept, dropped))
    }
}

fn entries(n: i64) -> &'static str {
    if n == 1 {
        "entry"
    } else {
        "entries"
    }
}
