//! Pixel sizes for the images in a document.
//!
//! An image block stores the media id, not its size, so the rendered `<img>`
//! had no `width` and `height` and the page shifted as pictures loaded. The
//! sizes are in the media table; this fills them into the blocks just before
//! rendering (the stored document is untouched).

use vyasa_core::block::{Block, BlockKind};

use crate::state::AppState;

/// Sets `width` and `height` on every image block that names a media item
/// and has no size of its own.
pub async fn resolve(state: &AppState, blocks: &mut [Block]) {
    let mut stack: Vec<&mut Block> = blocks.iter_mut().collect();
    while let Some(block) = stack.pop() {
        if block.kind == BlockKind::Image && needs_size(&block.attrs) {
            if let Some(id) = block
                .attrs
                .get("mediaId")
                .and_then(serde_json::Value::as_i64)
            {
                if let Ok(row) = state.media.get(id).await {
                    set_size(&mut block.attrs, row.width, row.height);
                }
            }
        }
        stack.extend(block.children.iter_mut());
    }
}

fn needs_size(attrs: &serde_json::Value) -> bool {
    let has = |key: &str| attrs.get(key).and_then(serde_json::Value::as_u64).is_some();
    !(has("width") && has("height"))
}

fn set_size(attrs: &mut serde_json::Value, width: Option<i32>, height: Option<i32>) {
    let (Some(w), Some(h)) = (width.filter(|w| *w > 0), height.filter(|h| *h > 0)) else {
        return;
    };
    if let Some(map) = attrs.as_object_mut() {
        map.insert("width".into(), w.into());
        map.insert("height".into(), h.into());
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_size_is_filled_in_only_when_the_block_has_none_and_the_media_has_both() {
        let mut a = json!({"mediaId": 3});
        assert!(needs_size(&a));
        set_size(&mut a, Some(640), Some(480));
        assert_eq!(a["width"], 640);
        assert_eq!(a["height"], 480);
        assert!(!needs_size(&a));
        let mut b = json!({"mediaId": 3});
        set_size(&mut b, Some(640), None);
        set_size(&mut b, Some(0), Some(5));
        assert!(b.get("width").is_none());
    }
}
