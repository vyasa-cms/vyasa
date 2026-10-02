//! Resolving plugin blocks before a document is rendered.
//!
//! The theme renderer is synchronous and knows nothing about plugins, and
//! calling into a sandbox is neither. So the application walks the
//! document first, asks each block's owning plugin to render it, and
//! stores the result on the block for the renderer to sanitize and emit.
//! A block whose plugin is gone renders as an HTML comment rather than
//! taking the post down with it.

use vyasa_core::{Block, BlockKind};
use vyasa_plugins::host::{HostEnv, V2Outcome};

use crate::state::AppState;

/// Most plugin blocks rendered for one page.
///
/// Each one is a fresh wasm instantiation, so a document with hundreds of
/// them would be a self-inflicted denial of service. Blocks past the cap
/// render as unresolved.
const MAX_PER_PAGE: usize = 32;

/// Whether this document contains anything to resolve.
///
/// Worth checking first: almost no document has a plugin block, and this
/// avoids taking the registry lock on every render.
#[must_use]
pub fn has_plugin_blocks(blocks: &[Block]) -> bool {
    blocks
        .iter()
        .any(|b| b.kind == BlockKind::Plugin || has_plugin_blocks(&b.children))
}

/// Renders every plugin block in the tree, in place.
pub async fn resolve(state: &AppState, blocks: &mut [Block]) {
    if !has_plugin_blocks(blocks) {
        return;
    }
    let mut budget = MAX_PER_PAGE;
    resolve_list(state, blocks, &mut budget).await;
}

/// Recursion as an explicit boxed future: `async fn` cannot recurse.
fn resolve_list<'a>(
    state: &'a AppState,
    blocks: &'a mut [Block],
    budget: &'a mut usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        for block in blocks.iter_mut() {
            if block.kind == BlockKind::Plugin {
                if *budget == 0 {
                    tracing::warn!("document exceeds {MAX_PER_PAGE} plugin blocks; rest skipped");
                    continue;
                }
                *budget -= 1;
                resolve_one(state, block).await;
            }
            if !block.children.is_empty() {
                resolve_list(state, &mut block.children, budget).await;
            }
        }
    })
}

async fn resolve_one(state: &AppState, block: &mut Block) {
    let Some(kind) = block.plugin_kind.clone() else {
        return;
    };
    let Some(decl) = state.plugin_blocks.get(&kind).await else {
        // Declared by a plugin that is no longer enabled. The renderer
        // emits a comment; the stored document is untouched, so
        // re-enabling the plugin brings the block back.
        return;
    };
    let attrs = block.attrs.to_string();
    let env = HostEnv::background(std::sync::Arc::clone(&state.broker), state.pool.clone());
    let html = match state
        .plugin_host
        .call_render_block(decl.plugin_id, &kind, &attrs, &env)
        .await
    {
        V2Outcome::Ok(html) => html,
        V2Outcome::Unsupported => {
            tracing::warn!(plugin = %decl.plugin_name, "declares {kind} but has no render-block");
            return;
        }
        V2Outcome::Failed(reason) => {
            tracing::warn!(plugin = %decl.plugin_name, "render {kind} failed: {reason}");
            state.metrics.inc_plugin_failure();
            return;
        }
    };
    // The renderer sanitizes this before it reaches a page; setting it
    // here is what tells the renderer the block resolved at all.
    if let Some(map) = block.attrs.as_object_mut() {
        map.insert(
            String::from(vyasa_themes::PLUGIN_BLOCK_RESOLVED_ATTR),
            serde_json::Value::String(html),
        );
    } else {
        block.attrs = serde_json::json!({ vyasa_themes::PLUGIN_BLOCK_RESOLVED_ATTR: html });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detection_reaches_nested_blocks() {
        let plain = vec![Block::new(BlockKind::Paragraph, json!({"text": "hi"}))];
        assert!(!has_plugin_blocks(&plain));

        let mut group = Block::new(BlockKind::Group, json!({}));
        group.children = vec![Block {
            kind: BlockKind::Plugin,
            plugin_kind: Some(String::from("acme/chart")),
            attrs: json!({}),
            children: Vec::new(),
        }];
        assert!(has_plugin_blocks(&[group]));
    }
}
