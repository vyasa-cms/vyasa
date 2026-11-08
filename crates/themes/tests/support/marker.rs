//! A stand-in plugin section and a typed tree, to render through any theme.
#![allow(dead_code, clippy::expect_used)]

use std::sync::Arc;

use vyasa_themes::{
    builtin_registry, render_page, BlockPayload, DynamicBlock, Engine, Layout, MapSettings,
    PageContext, RenderRequest, ResolveContext, Section, SiteMeta, TemplateType, TokenSet,
};

/// Stands in for a plugin section: proves a typed tree resolves plugin kinds.
pub struct Marker(pub &'static str);

impl DynamicBlock for Marker {
    fn kind(&self) -> &'static str {
        self.0
    }
    fn description(&self) -> &'static str {
        "test marker"
    }
    fn resolve<'a>(
        &'a self,
        _ctx: &'a ResolveContext<'a>,
        _settings: &'a MapSettings,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<BlockPayload, String>> + Send + 'a>,
    > {
        let html = format!("<nav>MARK:{}</nav>", self.0);
        Box::pin(std::future::ready(Ok(BlockPayload::Html(html))))
    }
}

/// Two columns: a plugin section on the left, the entry's body on the right.
pub fn tree() -> Vec<Section> {
    serde_json::from_str(
        r#"[{"id":"cols","kind":"columns","settings":{"count":2},"children":[
              {"id":"side","kind":"group","children":[{"id":"nav","kind":"test/marker"}]},
              {"id":"main","kind":"group","children":[{"id":"body","kind":"post-content"}]}]}]"#,
    )
    .expect("tree")
}

/// A `doc` entry that composes through `sections`.
pub fn page(sections: Vec<Section>) -> PageContext {
    PageContext {
        title: "Install".into(),
        post_title: Some("Install".into()),
        post_type_slug: Some("doc".into()),
        sections,
        regions_extra: vec![
            ("body".into(), "<p>BODY-HTML</p>".into()),
            ("content".into(), "<p>BODY-HTML</p>".into()),
        ],
        ..PageContext::default()
    }
}

/// Renders a single entry through `engine` with the marker registered.
pub fn render(engine: &Engine, layout: &Layout, tokens: &TokenSet, page: &PageContext) -> String {
    let mut registry = builtin_registry();
    registry.register(Arc::new(Marker("test/marker")));
    futures::executor::block_on(render_page(RenderRequest {
        engine,
        layout,
        registry: &registry,
        tokens,
        template: TemplateType::Single,
        site: &SiteMeta::default(),
        page,
        queries: crate::fake_queries::FakeQueries::shared(),
        post_id: Some(1),
        content_blocks: None,
        reply_to: None,
        editor: false,
    }))
    .expect("renders")
}
