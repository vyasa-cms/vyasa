//! Lessons both assistants have to keep.
//!
//! The theme studio and the page designer compose the same trees with the
//! same vocabulary, so a rule learned by watching one get it wrong belongs
//! in both prompts. Each of these cost a real model round to discover; this
//! is what stops one prompt being fixed and the other quietly regressing.
#![allow(clippy::expect_used)]

use vyasa_ai::{page_designer, theme_studio};

/// The system prompts of both assistants, with vocabulary embedded.
fn prompts() -> [(&'static str, String); 2] {
    let vocab = vyasa_themes::registry_schema();
    [
        ("theme studio", theme_studio::system_prompt(&vocab)),
        ("page designer", page_designer::system_prompt(&vocab)),
    ]
}

#[test]
fn both_say_a_repeating_section_is_one_list() {
    // Watched failure: asked for "three features", a model built three
    // `feature-grid` sections inside `columns`. Both render; one is right.
    for (who, prompt) in prompts() {
        assert!(
            prompt.contains("is one section, not three grids inside `columns`"),
            "{who} does not say a repeating section takes one list"
        );
    }
}

#[test]
fn both_say_what_contained_does() {
    // A "full-width band" that stops at the reading column is a rectangle
    // in the middle of the text; the flag that decides is not guessable.
    for (who, prompt) in prompts() {
        assert!(
            prompt.contains("spans the page unless you set \\\"contained\\\": true")
                || prompt.contains("spans the page unless you set \"contained\": true"),
            "{who} does not explain `contained`"
        );
    }
}

#[test]
fn both_warn_about_the_text_a_dark_band_forgets() {
    // Watched failure: a model set {bg: $text, text: $bg} and left
    // `text_muted` alone, so the sub-heading was 2.5:1 on near-black. The
    // validator warns, but the prompt should stop it happening.
    for (who, prompt) in prompts() {
        assert!(
            prompt.contains("set `text_muted` too"),
            "{who} does not warn about secondary text in a scope"
        );
    }
}

#[test]
fn both_know_a_section_can_be_withheld_from_a_screen_size() {
    // `hide_on` is not a setting on any kind, so nothing in the vocabulary
    // hints that it exists; without a line in the prompt the assistants
    // would compose two variants of a page instead.
    for (who, prompt) in prompts() {
        assert!(prompt.contains("`hide_on`"), "{who} never mentions hide_on");
        assert!(
            prompt.contains("noise on a phone"),
            "{who} does not say what it is for"
        );
    }
}

#[test]
fn both_put_the_brand_kit_above_their_own_taste() {
    // Without it a model has only "a blog about Rust" to go on, and every
    // site in a category has similar words — which is how you get the
    // median website for the category.
    let tokens = vyasa_themes::TokenSet::default();
    let brand = "- voice: Quiet and technical. No exclamation marks.";

    let designed = page_designer::user_prompt(
        &page_designer::DesignInput {
            vocabulary: &serde_json::Value::Null,
            site: "s",
            brand,
            title: "t",
            prose: "p",
            current: &[],
            message: "m",
            model: "m",
        },
        &tokens,
    );
    assert!(designed.contains(brand), "designer dropped it:\n{designed}");
    assert!(designed.contains("over any house style of your own"));

    let state = vyasa_themes::DraftState::from_json(
        &serde_json::json!({"version": 1}),
        &serde_json::to_value(default_layout()).expect("layout"),
        None,
    )
    .expect("state");
    let studio = theme_studio::user_prompt(
        &state,
        &theme_studio::AssistInput {
            vocabulary: &serde_json::Value::Null,
            site: "s",
            brand,
            history: &[],
            message: "m",
            attachments: Vec::new(),
            model: "m",
        },
    );
    assert!(studio.contains(brand), "studio dropped it:\n{studio}");
}

#[test]
fn an_empty_brand_kit_says_nothing_at_all() {
    // A heading with nothing under it is noise the model has to read past.
    let prompt = page_designer::user_prompt(
        &page_designer::DesignInput {
            vocabulary: &serde_json::Value::Null,
            site: "s",
            brand: "   ",
            title: "t",
            prose: "p",
            current: &[],
            message: "m",
            model: "m",
        },
        &vyasa_themes::TokenSet::default(),
    );
    assert!(!prompt.contains("brand kit"), "{prompt}");
}

/// A layout with something in every template, so `DraftState` accepts it.
fn default_layout() -> vyasa_themes::Layout {
    let mut l = vyasa_themes::Layout::default();
    for t in vyasa_themes::TemplateType::ALL {
        *l.for_template_mut(t) = vec![vyasa_themes::Section::new("body", "content")];
    }
    l
}

#[test]
fn both_mark_containers_and_carry_every_kind() {
    // Whether `children` is legal is not inferable from a settings schema.
    for (who, prompt) in prompts() {
        for kind in vyasa_themes::builtin_registry().kinds() {
            assert!(
                prompt.contains(&format!("- {kind}")),
                "{who} is missing {kind}"
            );
        }
        for container in ["band", "columns", "grid", "group"] {
            assert!(
                prompt.contains(&format!("- {container} [container")),
                "{who} does not mark {container} as a container"
            );
        }
    }
}

#[test]
fn both_scale_the_change_to_the_request() {
    // Watched failure: "make it magazine style" came back as a typeface and
    // a palette, revision 2, layout untouched. The prompts had asked for
    // "the smallest change that satisfies the request", which reads as a
    // licence to tweak when the request is a whole look -- and a magazine
    // is a shape before it is a serif. Keeping the restraint for a request
    // that names one property is still right; it just cannot be the only
    // instruction the model has.
    for (who, prompt) in prompts() {
        assert!(
            prompt.contains("Scale the change to the request"),
            "{who} still asks only for the smallest change"
        );
        assert!(
            prompt.contains("is a request to restyle"),
            "{who} does not say a whole-look request means rebuilding the layout"
        );
        assert!(
            prompt.contains("a shape, not a typeface"),
            "{who} does not say where an editorial look actually lives"
        );
        assert!(
            prompt.contains("did not reach"),
            "{who} dropped the restraint along with the rule that carried it"
        );
    }
}

#[test]
fn both_say_bindings_use_listed_sources_only() {
    // The failure this guards: a model asked for "the newest products"
    // inventing {"source": "products"} (plural) on a site whose plugin
    // registered "product". The section renders as nothing and the reply
    // reads like success.
    for (who, prompt) in prompts() {
        assert!(
            prompt.contains("Bind only to a source listed under `sources`"),
            "{who} does not pin bindings to listed sources"
        );
    }
}

#[test]
fn both_treat_plugin_kinds_as_vocabulary_only() {
    // The section registry is extensible: a commerce plugin brings
    // store/product-grid the day it is enabled. The flip side is a model
    // that has seen a thousand WooCommerce sites happily inventing
    // "woocommerce/products" on a site with no such plugin.
    for (who, prompt) in prompts() {
        assert!(
            prompt.contains("never invent a namespaced kind"),
            "{who} does not pin plugin kinds to the vocabulary"
        );
    }
}
