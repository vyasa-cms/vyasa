//! The site a brief runs against, without a server: the starting theme,
//! eyes that render the draft, and what the finished draft is judged on.

use std::collections::{BTreeMap, HashMap};
use std::future::{ready, Future};
use std::pin::Pin;

use sarathi_eval::structural::{Contrast, Proposal, PROPOSAL_VERSION};
use vyasa_ai::agent::AgentStep;
use vyasa_ai::theme_studio::StudioEyes;
use vyasa_core::menu::MenuDraft;
use vyasa_themes::{
    builtin_registry, contrast_ratio, render_page, validate_layout, validate_token_set,
    CommentNodeData, ContentQueries, DraftState, Engine, EntrySort, MonthArchiveData, PageContext,
    PageNodeData, PostCardData, RenderRequest, Section, SiteMeta, TemplateType, TermLinkData,
};

/// The starter theme each kind of site in the brief set begins from, by how
/// its context starts.
const STARTERS: &[(&str, Starter)] = &[
    ("A personal travel blog", Starter::Blog),
    ("A three-partner law firm", Starter::Blog),
    ("A charity site", Starter::Blog),
    ("A small online shop selling ceramics", Starter::Storefront),
    ("A developer documentation site", Starter::Docs),
    ("A B2B analytics product", Starter::Technology),
    ("A one-day conference site", Starter::Technology),
    ("A neighbourhood cafe site", Starter::Portfolio),
];

/// A starter theme shipped with Vyasa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Starter {
    /// `themes-starter/blog`.
    Blog,
    /// `themes-starter/docs`.
    Docs,
    /// `themes-starter/portfolio`.
    Portfolio,
    /// `themes-starter/storefront-lite`.
    Storefront,
    /// `themes-starter/technology`.
    Technology,
}

/// A starter's tokens and layout documents.
macro_rules! documents {
    ($dir:literal) => {
        (
            include_str!(concat!("../../../themes-starter/", $dir, "/tokens.json")),
            include_str!(concat!("../../../themes-starter/", $dir, "/layout.json")),
        )
    };
}

impl Starter {
    /// Every starter.
    pub const ALL: [Self; 5] = [
        Self::Blog,
        Self::Docs,
        Self::Portfolio,
        Self::Storefront,
        Self::Technology,
    ];

    /// The starter for a brief's site context.
    ///
    /// # Errors
    /// The context is of no known kind of site.
    pub fn for_site(context: &str) -> Result<Self, String> {
        STARTERS
            .iter()
            .find(|(start, _)| context.starts_with(start))
            .map(|(_, starter)| *starter)
            .ok_or_else(|| {
                let start: String = context.chars().take(60).collect();
                format!("no starter theme is set for the site context `{start}…`")
            })
    }

    /// The draft a run starts from.
    ///
    /// # Errors
    /// The shipped documents do not parse.
    pub fn draft(self) -> Result<DraftState, String> {
        let (tokens, layout) = match self {
            Self::Blog => documents!("blog"),
            Self::Docs => documents!("docs"),
            Self::Portfolio => documents!("portfolio"),
            Self::Storefront => documents!("storefront-lite"),
            Self::Technology => documents!("technology"),
        };
        let parse = |text: &str| serde_json::from_str(text).map_err(|e| format!("{self:?}: {e}"));
        DraftState::from_json(&parse(tokens)?, &parse(layout)?, None)
    }
}

/// The site's content: nothing published, and the staged menus.
struct Queries {
    menus: HashMap<String, String>,
}

type Listing<'a, T> = Pin<Box<dyn Future<Output = Result<Vec<T>, String>> + Send + 'a>>;

fn none<'a, T: Send + 'a>() -> Listing<'a, T> {
    Box::pin(ready(Ok(Vec::new())))
}

impl ContentQueries for Queries {
    fn entries<'a>(
        &'a self,
        _source: &'a str,
        _sort: EntrySort,
        _limit: u32,
        _term: Option<&'a str>,
    ) -> Listing<'a, PostCardData> {
        none()
    }
    fn categories(&self, _show_counts: bool) -> Listing<'_, TermLinkData> {
        none()
    }
    fn popular_tags(&self, _count: u32) -> Listing<'_, TermLinkData> {
        none()
    }
    fn monthly_archives(&self, _months: u32) -> Listing<'_, MonthArchiveData> {
        none()
    }
    fn navigation_menu<'a>(
        &'a self,
        slug: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        let html = self.menus.get(slug).cloned().unwrap_or_default();
        Box::pin(ready(Ok(html)))
    }
    fn page_tree(&self) -> Listing<'_, PageNodeData> {
        none()
    }
    fn approved_comments(&self, _post_id: i64) -> Listing<'_, CommentNodeData> {
        none()
    }
}

/// The studio's eyes without a server: `look` renders the draft with
/// Vyasa's renderer over an empty site and reads it back with the digest a
/// live `look` uses; `content` tells what the brief says about the site.
pub struct OfflineEyes {
    /// The brief's site context.
    pub context: String,
}

/// The template the public router would pick for `path`.
fn template_for(path: &str) -> TemplateType {
    let route = path.split_once('?').map_or(path, |(route, _)| route);
    let segments: Vec<&str> = route.split('/').filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        [] => TemplateType::Index,
        ["post", _] => TemplateType::Single,
        ["category" | "tag", _] => TemplateType::Archive,
        ["search"] => TemplateType::Search,
        [_] => TemplateType::Page,
        _ => TemplateType::NotFound,
    }
}

async fn render(sandbox: &DraftState, menus: &[MenuDraft], path: &str) -> Result<String, String> {
    let mut engine = Engine::builtin().map_err(|e| e.to_string())?;
    let overrides: Vec<(&str, &str)> = sandbox
        .templates
        .iter()
        .map(|(name, source)| (name.as_str(), source.as_str()))
        .collect();
    engine
        .install_theme_templates(&overrides)
        .map_err(|e| e.to_string())?;
    let queries = Queries {
        menus: menus
            .iter()
            .map(|m| (m.slug.clone(), m.render_html()))
            .collect(),
    };
    render_page(RenderRequest {
        engine: &engine,
        layout: &sandbox.layout,
        registry: &builtin_registry(),
        tokens: &sandbox.tokens,
        template: template_for(path),
        site: &SiteMeta::default(),
        page: &PageContext {
            title: "Preview".to_owned(),
            ..PageContext::default()
        },
        queries: &queries,
        post_id: None,
        content_blocks: None,
        reply_to: None,
        editor: false,
    })
    .await
    .map_err(|e| e.to_string())
}

impl StudioEyes for OfflineEyes {
    fn look<'a>(
        &'a mut self,
        sandbox: &'a DraftState,
        menus: &'a [MenuDraft],
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let html = render(sandbox, menus, path).await?;
            Ok(vyasa_themes::page_digest(&html))
        })
    }

    fn content<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(ready(Ok(self.context.clone())))
    }
}

/// The home page's sections as `kind` (`kind#2` for the second of a kind),
/// nested ones included.
fn home_sections(draft: &DraftState) -> Vec<String> {
    fn walk(sections: &[Section], seen: &mut BTreeMap<String, usize>, out: &mut Vec<String>) {
        for s in sections {
            let n = seen.entry(s.kind.clone()).or_default();
            *n += 1;
            out.push(if *n == 1 {
                s.kind.clone()
            } else {
                format!("{}#{n}", s.kind)
            });
            walk(&s.children, seen, out);
        }
    }
    let mut out = Vec::new();
    walk(
        draft.layout.for_template(TemplateType::Index),
        &mut BTreeMap::new(),
        &mut out,
    );
    out
}

/// What the structural check judges: the validator's errors on the finished
/// draft, the home sections it had and has, and the contrast of the text
/// colour pairs the token validator warns about.
#[must_use]
pub fn proposal(before: &DraftState, after: &DraftState) -> Proposal {
    let errors = validate_token_set(&after.tokens)
        .into_iter()
        .chain(validate_layout(&after.layout, &builtin_registry()))
        .filter(vyasa_themes::TokenDiagnostic::is_error)
        .map(|d| d.to_string())
        .collect();
    let c = &after.tokens.colors;
    let pairs = [
        ("text on bg", &c.text, &c.bg),
        ("text on surface", &c.text, &c.surface),
        ("text_muted on surface", &c.text_muted, &c.surface),
        ("on_primary on primary", &c.on_primary, &c.primary),
    ];
    let mut contrast = Vec::new();
    for (mode, dark) in [("light", false), ("dark", true)] {
        for (name, fg, bg) in pairs {
            let pick = |slot: &vyasa_themes::ColorSlot| {
                let dark_value = slot.dark.as_ref().filter(|_| dark);
                dark_value.unwrap_or(&slot.light).clone()
            };
            contrast.push(Contrast {
                pair: format!("{mode} {name}"),
                ratio: contrast_ratio(&pick(fg), &pick(bg)),
            });
        }
    }
    Proposal {
        proposal: PROPOSAL_VERSION,
        errors,
        required_sections: home_sections(before),
        sections: home_sections(after),
        contrast,
    }
}

/// `(valid, total)` operations of a run: each op of an `edit` call and each
/// `edit_menu` call, valid when the toolbox said it took effect.
#[must_use]
pub fn count_ops(steps: &[AgentStep]) -> (u32, u32) {
    let mut valid = 0u32;
    let mut total = 0u32;
    for step in steps {
        let (ops, took) = match step.tool.as_str() {
            "edit" => (
                step.input["ops"].as_array().map_or(1, Vec::len),
                step.observation.starts_with("Applied"),
            ),
            "edit_menu" => (1, step.observation.starts_with("Staged:")),
            _ => continue,
        };
        let ops = u32::try_from(ops).unwrap_or(u32::MAX);
        total = total.saturating_add(ops);
        if took {
            valid = valid.saturating_add(ops);
        }
    }
    (valid, total)
}
