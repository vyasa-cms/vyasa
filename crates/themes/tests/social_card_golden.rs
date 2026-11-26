//! Golden PNGs for the starter docs theme's social card: a short title,
//! a title that fills three lines, and one cut with an ellipsis. Pixels
//! are compared under a small tolerance; a change that moves them past it
//! must bump `vyasa_core::social_card::RENDERER_REVISION` and refresh the
//! goldens with `UPDATE_GOLDENS=1 cargo test -p vyasa-themes --test social_card_golden`.

#![allow(clippy::pedantic, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

const STARTER_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../themes-starter/docs");
const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/social_card");
/// The revision the goldens were drawn at.
const GOLDEN_REVISION: &str = "1";

const CASES: [(&str, &str, &str); 3] = [
    ("short", "Quickstart", "Guides"),
    (
        "three-lines",
        "Tuning shuffle partitions, broadcast joins and adaptive execution for large workloads",
        "Performance",
    ),
    (
        "ellipsis",
        "A very long page title that keeps going well past what three lines of the card can hold, \
         so the renderer has to cut it and end the last line with an ellipsis character",
        "Reference",
    ),
];

fn starter_card() -> vyasa_core::social_card::CardTheme {
    let manifest: toml::Table = toml::from_str(
        &std::fs::read_to_string(Path::new(STARTER_DIR).join("manifest.toml")).unwrap(),
    )
    .unwrap();
    let section = serde_json::to_value(&manifest["social_card"]).unwrap();
    vyasa_themes::package::load_social_card(&section, |path| {
        std::fs::read(Path::new(STARTER_DIR).join("assets").join(path)).ok()
    })
    .unwrap()
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(GOLDEN_DIR).join(format!("starter-docs-{name}.png"))
}

#[test]
fn the_goldens_match_the_renderer_revision() {
    assert_eq!(
        vyasa_core::social_card::RENDERER_REVISION,
        GOLDEN_REVISION,
        "the renderer revision moved: refresh the goldens (UPDATE_GOLDENS=1) and set GOLDEN_REVISION"
    );
}

#[test]
fn starter_docs_cards_match_the_goldens() {
    let card = starter_card();
    let update = std::env::var_os("UPDATE_GOLDENS").is_some();
    for (name, title, label) in CASES {
        assert!(
            card.supports(title, label),
            "{name}: the starter font draws it"
        );
        let png = card.render(title, label).unwrap();
        let path = golden_path(name);
        if update {
            std::fs::write(&path, &png).unwrap();
            continue;
        }
        let want = image::load_from_memory(&std::fs::read(&path).unwrap())
            .unwrap()
            .to_rgba8();
        let got = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(got.dimensions(), (1200, 630), "{name}");
        assert_eq!(got.dimensions(), want.dimensions(), "{name}");
        // Anti-aliasing may differ by a level or two across platforms; a
        // moved glyph changes hundreds of pixels by far more.
        let moved = got
            .pixels()
            .zip(want.pixels())
            .filter(|(a, b)| a.0.iter().zip(b.0.iter()).any(|(x, y)| x.abs_diff(*y) > 8))
            .count();
        let total = (got.width() * got.height()) as usize;
        assert!(
            moved * 10_000 <= total,
            "{name}: {moved} of {total} pixels differ from {} (over 0.01%)",
            path.display()
        );
    }
}

#[test]
fn the_three_cases_lay_out_as_named() {
    let card = starter_card();
    let lines = |t: &str| card.title_lines(t, "").unwrap();
    let short = lines(CASES[0].1);
    assert_eq!(short.len(), 1, "{short:?}");
    let three = lines(CASES[1].1);
    assert_eq!(three.len(), 3, "{three:?}");
    assert!(!three.last().unwrap().ends_with('…'), "{three:?}");
    let cut = lines(CASES[2].1);
    assert_eq!(cut.len(), 3, "{cut:?}");
    assert!(cut.last().unwrap().ends_with('…'), "{cut:?}");
}

/// Render time (layout, rasterise, PNG encode) on the starter theme. Run
/// with `cargo test --release -p vyasa-themes --test social_card_golden --
/// --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn render_time() {
    let card = starter_card();
    for (name, title, label) in CASES {
        let mut ms: Vec<f64> = (0..21)
            .map(|_| {
                let t = std::time::Instant::now();
                card.render(title, label).unwrap();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        ms.sort_by(f64::total_cmp);
        eprintln!("{name}: median {:.1} ms, max {:.1} ms", ms[10], ms[20]);
    }
}
